import hashlib
import json
import time

import pytest
from fastapi.testclient import TestClient

from dashboard.server import create_app


def client(files, public=False):
    origin = "https://stats.example.com" if public else "http://127.0.0.1:48763"
    return TestClient(create_app(files["auth"], files["snapshot"]), base_url=origin), origin


def login(c, origin, files):
    return c.post("/api/login", json={"username": "team", "password": files["password"]}, headers={"Origin": origin})


@pytest.mark.parametrize("path", ["/api/dashboard", "/api/session", "/snapshot.json", "/data/snapshot.json",
                                    "/private/auth.json", "/static/../private/auth.json", "/api/rpc", "/openapi.json", "/docs"])
def test_anonymous_cannot_read_data(web_files, sample, path):
    c, origin = client(web_files)
    response = c.get(path)
    assert response.status_code in {401, 404}
    assert sample["canary"] not in response.text
    assert "generated_at" not in response.text
    assert response.headers["cache-control"] == "no-store"


def test_login_read_logout_and_session_invalidation(web_files):
    c, origin = client(web_files)
    response = login(c, origin, web_files)
    assert response.status_code == 200
    csrf = response.json()["csrf"]
    token = c.cookies.get("local_dashboard_session")
    assert "HttpOnly" in response.headers["set-cookie"] and "SameSite=strict" in response.headers["set-cookie"]
    assert c.get("/api/dashboard").json()["data"]["totals"]["recorded"]["total_tokens"] == 750
    assert c.post("/api/logout", headers={"Origin": origin}).status_code == 403
    assert c.post("/api/logout", headers={"Origin": origin, "X-CSRF-Token": csrf}).status_code == 200
    c.cookies.set("local_dashboard_session", token)
    assert c.get("/api/dashboard").status_code == 401


def test_session_expiry_and_restart(web_files):
    c, origin = client(web_files)
    login(c, origin, web_files)
    token = c.cookies.get("local_dashboard_session")
    key = hashlib.sha256(token.encode()).hexdigest()
    expires, csrf, scope = c.app.state.auth.sessions[key]
    c.app.state.auth.sessions[key] = (time.monotonic()-1, csrf, scope)
    assert c.get("/api/dashboard").status_code == 401
    replacement, _ = client(web_files)
    replacement.cookies.set("local_dashboard_session", token)
    assert replacement.get("/api/dashboard").status_code == 401


def test_https_cookie_cannot_be_downgraded_or_used_on_another_host(web_files):
    c, origin = client(web_files, public=True)
    response = login(c, origin, web_files)
    assert "__Host-dashboard_session=" in response.headers["set-cookie"]
    assert "Secure" in response.headers["set-cookie"] and "Domain=" not in response.headers["set-cookie"]
    token = c.cookies.get("__Host-dashboard_session")
    assert c.app.state.auth.lookup(token, "127.0.0.1:48763") is None
    assert c.get("/api/dashboard", headers={"X-Forwarded-Proto": "http"}).status_code == 200


@pytest.mark.parametrize("origin", ["https://evil.example", "null", "https://stats.example.com.evil", ""])
def test_login_origin_rejected(web_files, origin):
    c, _ = client(web_files)
    response = c.post("/api/login", headers={"Origin": origin}, json={"username": "team", "password": web_files["password"]})
    assert response.status_code == 403


def test_login_limits_do_not_trust_forwarded_ip(web_files):
    c, origin = client(web_files)
    c.app.state.auth.verify = lambda *args: False
    for index in range(10):
        response = c.post("/api/login", json={"username": "team", "password": "wrong"}, headers={"Origin": origin, "X-Forwarded-For": f"1.2.3.{index}"})
        assert response.status_code == 401
    response = login(c, origin, web_files)
    assert response.status_code == 429 and response.headers["Retry-After"] == "60"


def test_no_management_routes_or_arbitrary_parameters(web_files):
    c, origin = client(web_files)
    login(c, origin, web_files)
    for path in ("/api/rpc", "/apikey/readSecret", "/api/export", "/api/users", "/api/keys/id",
                 "/api/accounts/id/reset", "/api/accounts/id/sort", "/api/codex/rate-limit-reset-credits/consume"):
        assert c.post(path, json={"method": "apikey/readSecret"}).status_code == 404
    assert c.get("/api/dashboard?key_id=someone-else").status_code == 400
    assert c.delete("/api/dashboard").status_code == 405
    assert c.get("/api/dashboard", headers={"Host": "evil.example"}).status_code == 400


@pytest.mark.parametrize("damage", ["secret", "invalid", "large", "expired", "future"])
def test_snapshot_fails_closed_without_error_echo(web_files, damage):
    c, origin = client(web_files)
    login(c, origin, web_files)
    data = json.loads(web_files["snapshot"].read_text(encoding="utf-8"))
    if damage == "secret":
        data["access_token"] = "CANARY_TOKEN_DO_NOT_ECHO"
    elif damage == "expired":
        data["generated_at"] = int(time.time())-90000
    elif damage == "future":
        data["generated_at"] = int(time.time())+1000
    content = "{" if damage == "invalid" else "x" * (4*1024*1024+1) if damage == "large" else json.dumps(data)
    web_files["snapshot"].write_text(content, encoding="utf-8")
    response = c.get("/api/dashboard")
    assert response.status_code == 503
    assert "CANARY" not in response.text and "ValidationError" not in response.text


def test_stale_snapshot_is_labeled_not_zeroed(web_files):
    c, origin = client(web_files)
    login(c, origin, web_files)
    data = json.loads(web_files["snapshot"].read_text(encoding="utf-8"))
    data["generated_at"] = int(time.time())-400
    web_files["snapshot"].write_text(json.dumps(data), encoding="utf-8")
    response = c.get("/api/dashboard").json()
    assert response["stale"] is True
    assert response["data"]["totals"]["recorded"]["total_tokens"] == 750


def test_oversized_login_and_csp(web_files):
    c, origin = client(web_files)
    response = c.post("/api/login", content=b"x"*3000, headers={"Origin": origin, "Content-Type": "application/json"})
    assert response.status_code == 413
    assert c.get("/").status_code == 200
    assert "frame-ancestors 'none'" in c.get("/").headers["content-security-policy"]
    assert "access-control-allow-origin" not in c.get("/").headers


def test_surrogate_and_non_ascii_csrf_are_rejected(web_files):
    c, origin = client(web_files)
    response = c.post("/api/login", content=b'{"username":"team","password":"\\ud800"}',
                      headers={"Origin": origin, "Content-Type": "application/json"})
    assert response.status_code == 400
    login(c, origin, web_files)
    response = c.post("/api/logout", headers={"Origin": origin, "X-CSRF-Token": b"\xff"})
    assert response.status_code == 403


def test_kdf_parallelism_is_bounded(web_files):
    import asyncio
    import threading
    import httpx
    app = create_app(web_files["auth"], web_files["snapshot"])
    active, maximum = 0, 0
    lock = threading.Lock()

    def verify(*args):
        nonlocal active, maximum
        with lock:
            active += 1
            maximum = max(maximum, active)
        time.sleep(.05)
        with lock:
            active -= 1
        return False

    app.state.auth.verify = verify

    async def exercise():
        async with httpx.AsyncClient(transport=httpx.ASGITransport(app=app), base_url="http://127.0.0.1:48763") as c:
            requests = [c.post("/api/login", json={"username": "a", "password": "b"}, headers={"Origin": "http://127.0.0.1:48763"}) for _ in range(6)]
            assert all(response.status_code == 401 for response in await asyncio.gather(*requests))

    asyncio.run(exercise())
    assert maximum == 2 and active == 0


def tunnel_client(files, base_url="https://stats.example.com"):
    config = json.loads(files["auth"].read_text(encoding="utf-8"))
    config["cloudflare_tunnel_origins"] = ["https://stats.example.com"]
    files["auth"].write_text(json.dumps(config), encoding="utf-8")
    return TestClient(create_app(files["auth"], files["snapshot"]), base_url=base_url)


@pytest.mark.parametrize("value", [
    None, "https://stats.example.com", {}, [42], ["http://127.0.0.1:48763"],
    ["https://unapproved.example"], ["https://stats.example.com/"],
    ["https://stats.example.com?query=1"], ["https://stats.example.com#fragment"],
])
def test_tunnel_origin_configuration_fails_closed(web_files, value):
    config = json.loads(web_files["auth"].read_text(encoding="utf-8"))
    config["cloudflare_tunnel_origins"] = value
    web_files["auth"].write_text(json.dumps(config), encoding="utf-8")
    with pytest.raises(RuntimeError, match="Cloudflare Tunnel origins"):
        create_app(web_files["auth"], web_files["snapshot"])


def test_tunnel_origin_cannot_contain_userinfo_even_if_allowlisted(web_files):
    config = json.loads(web_files["auth"].read_text(encoding="utf-8"))
    origin = "https://user:password@stats.example.com"
    config["origins"].append(origin)
    config["cloudflare_tunnel_origins"] = [origin]
    web_files["auth"].write_text(json.dumps(config), encoding="utf-8")
    with pytest.raises(RuntimeError, match="Cloudflare Tunnel origins"):
        create_app(web_files["auth"], web_files["snapshot"])


@pytest.mark.parametrize("headers", [
    [], [("X-Forwarded-Proto", "")], [("X-Forwarded-Proto", "HTTPS")],
    [("X-Forwarded-Proto", "https ")], [("X-Forwarded-Proto", "ftp")],
    [("X-Forwarded-Proto", "https, http")],
    [("X-Forwarded-Proto", "https"), ("X-Forwarded-Proto", "https")],
    [("X-Forwarded-Proto", "http"), ("X-Forwarded-Proto", "https")],
])
def test_tunnel_requires_exactly_one_strict_scheme_header(web_files, headers):
    c = tunnel_client(web_files)
    response = c.get("/", headers=headers, follow_redirects=False)
    assert response.status_code == 400
    assert response.json() == {"error": "invalid_forwarded_proto"}
    assert "location" not in response.headers and "set-cookie" not in response.headers
    assert response.headers["cache-control"] == "no-store"
    assert "frame-ancestors 'none'" in response.headers["content-security-policy"]
    assert response.headers["strict-transport-security"] == "max-age=31536000"


@pytest.mark.parametrize("method", ["GET", "HEAD"])
@pytest.mark.parametrize("path", ["/", "/static/app.js", "/%2F%2Fevil.example/%0d%0aInjected%3Ayes"])
def test_tunnel_http_redirects_only_to_configured_https_host(web_files, method, path):
    from urllib.parse import urlsplit
    c = tunnel_client(web_files, base_url="http://stats.example.com")
    response = c.request(method, path, headers={
        "X-Forwarded-Proto": "http", "X-Forwarded-Host": "evil.example",
        "Forwarded": 'proto=https;host="evil.example"',
    }, follow_redirects=False)
    assert response.status_code == 308
    location = response.headers["location"]
    assert urlsplit(location).scheme == "https"
    assert urlsplit(location).netloc == "stats.example.com"
    assert "\r" not in location and "\n" not in location
    assert "set-cookie" not in response.headers
    assert response.headers["cache-control"] == "no-store"
    if method == "HEAD":
        assert response.content == b""


@pytest.mark.parametrize("method", ["POST", "PUT", "PATCH", "DELETE", "OPTIONS"])
def test_tunnel_http_non_read_methods_rejected_before_body_read(web_files, method):
    import asyncio
    import httpx
    c = tunnel_client(web_files)
    c.app.state.auth.verify = lambda *args: pytest.fail("HTTP must not verify credentials")

    async def unread_body():
        raise AssertionError("HTTP request body must not be read")
        yield b"unreachable"

    async def exercise():
        async with httpx.AsyncClient(transport=httpx.ASGITransport(app=c.app),
                                     base_url="http://stats.example.com") as request_client:
            response = await request_client.request(method, "/api/login", content=unread_body(), headers={
                "X-Forwarded-Proto": "http", "Origin": "https://stats.example.com",
                "Content-Type": "application/json",
            })
            assert response.status_code == 400
            assert response.json() == {"error": "https_required"}
            assert "set-cookie" not in response.headers and "location" not in response.headers
            assert response.headers["cache-control"] == "no-store"

    asyncio.run(exercise())
    assert not c.app.state.auth.global_attempts


def test_tunnel_https_login_session_and_logout_keep_secure_cookie(web_files):
    c = tunnel_client(web_files)
    c.headers["X-Forwarded-Proto"] = "https"
    response = login(c, "https://stats.example.com", web_files)
    assert response.status_code == 200
    assert "__Host-dashboard_session=" in response.headers["set-cookie"]
    assert "Secure" in response.headers["set-cookie"]
    assert "HttpOnly" in response.headers["set-cookie"]
    assert "SameSite=strict" in response.headers["set-cookie"]
    assert "Domain=" not in response.headers["set-cookie"]
    assert c.get("/api/dashboard").status_code == 200
    assert c.post("/api/logout", headers={"Origin": "https://stats.example.com",
                  "X-CSRF-Token": response.json()["csrf"]}).status_code == 200
    assert c.get("/api/dashboard").status_code == 401


def test_tunnel_scheme_trust_is_opt_in_and_does_not_break_local_login(web_files):
    c = tunnel_client(web_files, base_url="http://127.0.0.1:48763")
    c.headers["X-Forwarded-Proto"] = "invalid-and-ignored-for-local"
    response = login(c, "http://127.0.0.1:48763", web_files)
    assert response.status_code == 200
    assert "local_dashboard_session=" in response.headers["set-cookie"]
    assert "Secure" not in response.headers["set-cookie"]
    assert c.get("/api/dashboard").status_code == 200


@pytest.mark.parametrize("host", ["evil.example", "stats.example.com.evil", "stats.example.com@evil.example"])
def test_tunnel_rejects_unapproved_hosts_without_redirect(web_files, host):
    c = tunnel_client(web_files)
    response = c.get("/", headers={"Host": host, "X-Forwarded-Proto": "http"}, follow_redirects=False)
    assert response.status_code == 400
    assert response.json() == {"error": "host_not_allowed"}
    assert "location" not in response.headers
    assert response.headers["cache-control"] == "no-store"


def test_tunnel_rejects_duplicate_host_and_query_redirect_parameters(web_files):
    c = tunnel_client(web_files)
    response = c.get("/", headers=[("Host", "stats.example.com"), ("Host", "evil.example"),
                                   ("X-Forwarded-Proto", "http")], follow_redirects=False)
    assert response.status_code == 400 and "location" not in response.headers
    response = c.get("/?next=https://evil.example", headers={"X-Forwarded-Proto": "http"}, follow_redirects=False)
    assert response.status_code == 400 and "location" not in response.headers


def test_tunnel_does_not_bypass_origin_or_trust_forwarded_client_ip(web_files):
    c = tunnel_client(web_files)
    c.headers["X-Forwarded-Proto"] = "https"
    assert login(c, "https://evil.example", web_files).status_code == 403
    c.app.state.auth.verify = lambda *args: False
    for index in range(10):
        response = c.post("/api/login", json={"username": "team", "password": "wrong"}, headers={
            "Origin": "https://stats.example.com", "X-Forwarded-For": f"1.2.3.{index}",
            "CF-Connecting-IP": f"5.6.7.{index}", "Forwarded": f"for=9.10.11.{index};proto=https",
        })
        assert response.status_code == 401
    assert login(c, "https://stats.example.com", web_files).status_code == 429


def test_tunnel_header_does_not_mutate_asgi_scheme_host_or_client(web_files):
    from fastapi import Request
    c = tunnel_client(web_files, base_url="http://stats.example.com")

    @c.app.get("/test-proxy-trust")
    def inspect_scope(request: Request):
        return {"scheme": request.url.scheme, "host": request.url.hostname, "client": request.client.host}

    response = c.get("/test-proxy-trust", headers={
        "X-Forwarded-Proto": "https", "X-Forwarded-For": "1.2.3.4",
        "CF-Connecting-IP": "5.6.7.8", "X-Forwarded-Host": "evil.example",
    })
    assert response.status_code == 200
    assert response.json() == {"scheme": "http", "host": "stats.example.com", "client": "testclient"}
