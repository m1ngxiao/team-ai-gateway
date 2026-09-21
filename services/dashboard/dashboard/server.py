import asyncio
import json
import logging
import os
import re
import time
from pathlib import Path
from urllib.parse import quote, urlsplit

from fastapi import FastAPI, HTTPException, Request
from fastapi.responses import FileResponse, JSONResponse, RedirectResponse
from pydantic import ValidationError
from starlette.concurrency import run_in_threadpool

from .models import Snapshot
from .security import Authentication

LOG = logging.getLogger("dashboard")
STATIC = Path(__file__).with_name("static")
MAX_BODY = 2048
MAX_SNAPSHOT = 4 * 1024 * 1024


def create_app(config_path: Path | None = None, snapshot_path: Path | None = None) -> FastAPI:
    config_path = config_path or Path(os.environ.get("DASHBOARD_AUTH_FILE", "/run/dashboard/auth.json"))
    snapshot_path = snapshot_path or Path(os.environ.get("DASHBOARD_SNAPSHOT", "/data/snapshot.json"))
    config = json.loads(config_path.read_text(encoding="utf-8"))
    origins = config.get("origins", [])
    if not origins or any(
        urlsplit(origin).scheme not in {"http", "https"}
        or not urlsplit(origin).netloc or urlsplit(origin).path
        or (urlsplit(origin).scheme == "http" and urlsplit(origin).hostname not in {"localhost", "127.0.0.1"})
        for origin in origins
    ):
        raise RuntimeError("Only exact HTTPS origins or local loopback HTTP origins are allowed")
    tunnel_origins = config.get("cloudflare_tunnel_origins", [])
    if not isinstance(tunnel_origins, list) or any(
        not isinstance(origin, str) or origin not in origins
        or urlsplit(origin).scheme != "https"
        or origin != f"https://{urlsplit(origin).netloc}"
        or urlsplit(origin).username is not None
        or urlsplit(origin).password is not None
        for origin in tunnel_origins
    ):
        raise RuntimeError("Cloudflare Tunnel origins must be exact HTTPS origins from origins")
    tunnel_hosts = {urlsplit(origin).netloc.lower(): origin for origin in tunnel_origins}
    if not re.fullmatch(r"[a-f0-9]{32}", config.get("salt", "")) or not re.fullmatch(
        r"[a-f0-9]{64}", config.get("password_hash", "")
    ):
        raise RuntimeError("Initialize a password hash before starting")
    auth = Authentication(config["username"], config["salt"], config["password_hash"])
    allowed_hosts = {urlsplit(origin).netloc.lower() for origin in origins}
    app = FastAPI(docs_url=None, redoc_url=None, openapi_url=None)
    app.state.auth = auth
    kdf_slots = asyncio.Semaphore(2)

    def cookie_name(request):
        return "__Host-dashboard_session" if f"https://{request.headers['host'].lower()}" in origins else "local_dashboard_session"

    @app.middleware("http")
    async def guard(request: Request, call_next):
        hosts = request.headers.getlist("host")
        host = hosts[0].lower() if len(hosts) == 1 else ""
        response = None
        if host not in allowed_hosts:
            response = JSONResponse({"error": "host_not_allowed"}, status_code=400)
        elif request.url.query:
            response = JSONResponse({"error": "query_not_allowed"}, status_code=400)
        elif host in tunnel_hosts:
            # Explicit deployment opt-in only: Cloudflare overwrites this header at
            # its edge, and this origin must remain reachable only through the
            # trusted local Tunnel. Never enable generic proxy-header/IP trust.
            schemes = request.headers.getlist("x-forwarded-proto")
            if len(schemes) != 1 or schemes[0] not in {"http", "https"}:
                response = JSONResponse({"error": "invalid_forwarded_proto"}, status_code=400)
            elif schemes[0] == "http":
                if request.method in {"GET", "HEAD"}:
                    # The authority is configured, never supplied by forwarding
                    # headers or the request path. Encode control characters too.
                    target = tunnel_hosts[host] + quote(request.url.path, safe="/")
                    response = RedirectResponse(target, status_code=308)
                else:
                    # Do not read credentials, issue cookies, or replay a body
                    # that was submitted over the client's cleartext connection.
                    response = JSONResponse({"error": "https_required"}, status_code=400)
        if response is None:
            response = await call_next(request)
        response.headers.update({
            "Cache-Control": "no-store",
            "X-Content-Type-Options": "nosniff",
            "Referrer-Policy": "no-referrer",
            "X-Frame-Options": "DENY",
            "Content-Security-Policy": "default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self'; connect-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'",
            "Permissions-Policy": "camera=(), microphone=(), geolocation=()",
        })
        if f"https://{host}" in origins:
            response.headers["Strict-Transport-Security"] = "max-age=31536000"
        return response

    def session(request: Request) -> str:
        csrf = auth.lookup(request.cookies.get(cookie_name(request)), request.headers["host"].lower())
        if not csrf:
            raise HTTPException(401, "login_required")
        return csrf

    def require_origin(request: Request) -> None:
        origin = request.headers.get("origin", "")
        if origin not in origins or urlsplit(origin).netloc.lower() != request.headers.get("host", "").lower():
            raise HTTPException(403, "origin_not_allowed")

    @app.get("/healthz")
    def health():
        return {"status": "ok"}

    @app.get("/")
    def index():
        return FileResponse(STATIC / "index.html")

    @app.get("/static/{filename}")
    def asset(filename: str):
        if filename not in {"app.css", "app.js"}:
            raise HTTPException(404)
        return FileResponse(STATIC / filename)

    @app.post("/api/login")
    async def login(request: Request):
        require_origin(request)
        address = request.client.host if request.client else "unknown"
        # Deliberately ignore X-Forwarded-For / CF-Connecting-IP: no spoofable trust chain.
        if not auth.allow_attempt(address):
            raise HTTPException(429, "try_again_later", headers={"Retry-After": "60"})
        if request.headers.get("content-type", "").split(";")[0].strip() != "application/json":
            raise HTTPException(415, "json_required")
        raw = bytearray()
        try:
            async with asyncio.timeout(5):
                async for chunk in request.stream():
                    raw.extend(chunk)
                    if len(raw) > MAX_BODY:
                        raise HTTPException(413, "request_too_large")
        except TimeoutError:
            raise HTTPException(408, "body_timeout") from None
        try:
            body = json.loads(raw)
            if not isinstance(body, dict) or set(body) != {"username", "password"}:
                raise ValueError()
            username, password = body["username"], body["password"]
            if not isinstance(username, str) or not isinstance(password, str) or len(username) > 64 or len(password) > 256:
                raise ValueError()
            username.encode("utf-8")
            password.encode("utf-8")
        except (ValueError, KeyError, TypeError, UnicodeError):
            raise HTTPException(400, "invalid_login") from None
        try:
            await asyncio.wait_for(kdf_slots.acquire(), timeout=3)
        except TimeoutError:
            raise HTTPException(429, "server_busy", headers={"Retry-After": "5"}) from None
        try:
            verified = await run_in_threadpool(auth.verify, username, password)
        finally:
            kdf_slots.release()
        if not verified:
            LOG.info("login_rejected")
            raise HTTPException(401, "invalid_credentials")
        token, csrf = auth.issue(request.headers["host"].lower())
        response = JSONResponse({"csrf": csrf})
        secure = f"https://{request.headers['host'].lower()}" in origins
        response.set_cookie(cookie_name(request), token, httponly=True, secure=secure,
                            samesite="strict", max_age=auth.ttl, path="/")
        LOG.info("login_accepted")
        return response

    @app.get("/api/session")
    def session_info(request: Request):
        return {"csrf": session(request), "role": "viewer"}

    @app.post("/api/logout")
    def logout(request: Request):
        require_origin(request)
        csrf = session(request)
        import hmac
        if not hmac.compare_digest(request.headers.get("x-csrf-token", "").encode(), csrf.encode()):
            raise HTTPException(403, "csrf_invalid")
        auth.revoke(request.cookies.get(cookie_name(request)))
        response = JSONResponse({"ok": True})
        response.delete_cookie(cookie_name(request), path="/", secure=cookie_name(request).startswith("__Host-"), httponly=True, samesite="strict")
        return response

    @app.get("/api/dashboard")
    def dashboard(request: Request):
        session(request)
        try:
            with snapshot_path.open("rb") as handle:
                raw = handle.read(MAX_SNAPSHOT + 1)
            if len(raw) > MAX_SNAPSHOT:
                raise ValueError("snapshot too large")
            data = Snapshot.model_validate_json(raw)
            age = int(time.time()) - data.generated_at
            if age < -120:
                raise ValueError("future snapshot")
        except (OSError, ValueError, ValidationError):
            # Never expose parser/validation errors: they can contain the offending secret.
            LOG.warning("snapshot_unavailable")
            raise HTTPException(503, "snapshot_unavailable") from None
        if age > 86400:
            raise HTTPException(503, "snapshot_expired")
        return {"data": data.model_dump(), "stale": age > 180, "age_seconds": max(age, 0)}

    return app
