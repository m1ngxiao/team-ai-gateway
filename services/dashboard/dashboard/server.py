import asyncio
import hashlib
import hmac
import http.client
import json
import logging
import os
import re
import time
from pathlib import Path
from urllib import request as urlrequest
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
MAX_SPEED_BODY = 8192
DEFAULT_SPEEDTEST_URL = "http://10.10.2.10:48760/v1/responses"
GATEWAY_METRICS_URL = os.environ.get("DASHBOARD_GATEWAY_METRICS_URL", "http://10.10.2.10:48760/metrics")
_ACCOUNT_INFLIGHT_RE = re.compile(r'^codexmanager_gateway_account_inflight\{account_id="([^"]+)"\}\s+(\d+)')


def _public_account_id(raw_id, secret):
    digest = hmac.new(bytes.fromhex(secret), f"acct:{raw_id}".encode(), hashlib.sha256).hexdigest()
    return f"acct-{digest[:12]}"


def collect_live_inflight(id_secret=None):
    try:
        with urlrequest.urlopen(GATEWAY_METRICS_URL, timeout=1.5) as response:
            text = response.read(1_000_000).decode("utf-8", "replace")
    except Exception:
        return {"accounts": {}, "total": 0, "available": False, "at": int(time.time())}
    accounts = {}
    for line in text.splitlines():
        match = _ACCOUNT_INFLIGHT_RE.match(line.strip())
        if match:
            account_id = _public_account_id(match.group(1), id_secret) if id_secret else match.group(1)
            accounts[account_id] = int(match.group(2))
    return {"accounts": accounts, "total": sum(accounts.values()), "available": True, "at": int(time.time())}


def run_speedtest(url, api_key, model, prompt):
    """Run a small streaming request and measure visible-output tok/s."""
    parsed = urlsplit(url)
    connection_class = http.client.HTTPSConnection if parsed.scheme == "https" else http.client.HTTPConnection
    port = parsed.port or (443 if parsed.scheme == "https" else 80)
    connection = connection_class(parsed.hostname, port, timeout=150)
    path = parsed.path or "/v1/responses"
    if parsed.query:
        path += "?" + parsed.query
    payload = json.dumps({
        "model": model,
        "input": prompt,
        "stream": True,
        "max_output_tokens": 256,
    }).encode("utf-8")
    started = time.monotonic()
    try:
        connection.request("POST", path, payload, {
            "Authorization": "Bearer " + api_key,
            "Content-Type": "application/json",
            "Accept": "text/event-stream",
            "User-Agent": "team-gateway-dashboard-speedtest/1",
        })
        response = connection.getresponse()
        if response.status != 200:
            detail = response.read(2048).decode("utf-8", "replace").strip()
            raise ValueError("HTTP %d: %s" % (response.status, detail[:300]))
        first_delta_ms = None
        text = []
        output_tokens = 0
        reasoning_tokens = 0
        for raw in response:
            line = raw.decode("utf-8", "replace").strip()
            if not line.startswith("data:"):
                continue
            data = line[5:].strip()
            if not data or data == "[DONE]":
                continue
            try:
                event = json.loads(data)
            except ValueError:
                continue
            kind = event.get("type")
            if kind == "response.output_text.delta":
                if first_delta_ms is None:
                    first_delta_ms = (time.monotonic() - started) * 1000
                delta = event.get("delta") or ""
                if delta:
                    text.append(delta)
            elif kind == "response.completed":
                usage = (event.get("response") or {}).get("usage") or {}
                output_tokens = max(output_tokens, int(usage.get("output_tokens") or 0))
                details = usage.get("output_tokens_details") or {}
                reasoning_tokens = max(reasoning_tokens, int(details.get("reasoning_tokens") or 0))
            elif kind == "response.failed":
                error = (event.get("response") or {}).get("error") or {}
                message = str(error.get("message") or error.get("code") or "upstream failed")
                raise ValueError(message[:300])
        duration_ms = (time.monotonic() - started) * 1000
        joined = "".join(text)
        if output_tokens <= 0:
            output_tokens = max(1, len(joined) // 3)
        visible_tokens = max(output_tokens - reasoning_tokens, 0)
        if visible_tokens <= 0:
            visible_tokens = output_tokens
        if first_delta_ms is None:
            first_delta_ms = duration_ms
        decode_ms = max(1.0, duration_ms - first_delta_ms)
        return {
            "model": model,
            "output_tokens": output_tokens,
            "visible_tokens": visible_tokens,
            "first_delta_ms": round(first_delta_ms, 1),
            "duration_ms": round(duration_ms, 1),
            "e2e_tps": round(output_tokens / max(duration_ms / 1000, 0.001), 1),
            "decode_tps": round(visible_tokens / (decode_ms / 1000), 1),
            "text": joined[:600],
        }
    finally:
        connection.close()


def create_app(config_path: Path | None = None, snapshot_path: Path | None = None) -> FastAPI:
    config_path = config_path or Path(os.environ.get("DASHBOARD_AUTH_FILE", "/run/dashboard/auth.json"))
    snapshot_path = snapshot_path or Path(os.environ.get("DASHBOARD_SNAPSHOT", "/data/snapshot.json"))
    config = json.loads(config_path.read_text(encoding="utf-8"))
    collector_config_path = Path(os.environ.get("DASHBOARD_COLLECTOR_CONFIG", "/run/dashboard/collector.json"))
    try:
        collector_config = json.loads(collector_config_path.read_text(encoding="utf-8"))
        id_secret = collector_config.get("id_secret")
        if not isinstance(id_secret, str) or not re.fullmatch(r"[a-f0-9]{64}", id_secret):
            id_secret = None
    except (OSError, ValueError):
        id_secret = None
    origins = config.get("origins", [])
    speedtest_url = os.environ.get("DASHBOARD_SPEEDTEST_URL", DEFAULT_SPEEDTEST_URL)
    parsed_speedtest = urlsplit(speedtest_url)
    if (
        parsed_speedtest.scheme not in {"http", "https"}
        or not parsed_speedtest.hostname
        or parsed_speedtest.username is not None
        or parsed_speedtest.password is not None
    ):
        raise RuntimeError("DASHBOARD_SPEEDTEST_URL must be a plain HTTP(S) URL")
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
    app.state.id_secret = id_secret
    kdf_slots = asyncio.Semaphore(2)
    speedtest_slots = asyncio.Semaphore(1)

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

    @app.post("/api/speedtest")
    async def speedtest(request: Request):
        require_origin(request)
        csrf = session(request)
        if not hmac.compare_digest(request.headers.get("x-csrf-token", "").encode(), csrf.encode()):
            raise HTTPException(403, "csrf_invalid")
        if request.headers.get("content-type", "").split(";")[0].strip() != "application/json":
            raise HTTPException(415, "json_required")
        raw = bytearray()
        try:
            async with asyncio.timeout(5):
                async for chunk in request.stream():
                    raw.extend(chunk)
                    if len(raw) > MAX_SPEED_BODY:
                        raise HTTPException(413, "request_too_large")
        except TimeoutError:
            raise HTTPException(408, "body_timeout") from None
        try:
            body = json.loads(raw)
            if not isinstance(body, dict) or set(body) != {"api_key", "model", "prompt"}:
                raise ValueError()
            api_key, model, prompt = body["api_key"], body["model"], body["prompt"]
            if not isinstance(api_key, str) or not re.fullmatch(r"[A-Za-z0-9._-]{8,200}", api_key):
                raise ValueError()
            if not isinstance(model, str) or not re.fullmatch(r"[a-zA-Z0-9][a-zA-Z0-9._-]{0,63}", model):
                raise ValueError()
            if not isinstance(prompt, str) or not 1 <= len(prompt.strip()) <= 2000:
                raise ValueError()
        except (ValueError, KeyError, TypeError, UnicodeError):
            raise HTTPException(400, "invalid_speedtest") from None
        try:
            await asyncio.wait_for(speedtest_slots.acquire(), timeout=1)
        except TimeoutError:
            raise HTTPException(429, "speedtest_busy", headers={"Retry-After": "3"}) from None
        try:
            return await run_in_threadpool(run_speedtest, speedtest_url, api_key, model, prompt.strip())
        except ValueError as error:
            raise HTTPException(502, str(error)[:320]) from None
        finally:
            speedtest_slots.release()

    @app.get("/api/live")
    def live(request: Request):
        session(request)
        return collect_live_inflight(app.state.id_secret)

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
