"""Single-worker, bounded sessions and login throttling. No platform API keys."""

import hashlib
import hmac
import secrets
import threading
import time
from collections import OrderedDict, deque


def password_hash(password: str, salt: str) -> str:
    return hashlib.scrypt(
        password.encode("utf-8"), salt=bytes.fromhex(salt), n=32768, r=8, p=1,
        maxmem=64 * 1024 * 1024, dklen=32,
    ).hex()


class Authentication:
    def __init__(self, username: str, salt: str, digest: str, ttl: int = 8 * 3600):
        self.username, self.salt, self.digest, self.ttl = username, salt, digest, ttl
        self.sessions: OrderedDict[str, tuple[float, str, str]] = OrderedDict()
        self.attempts: OrderedDict[str, deque[float]] = OrderedDict()
        self.global_attempts: deque[float] = deque()
        self.lock = threading.Lock()

    def allow_attempt(self, address: str) -> bool:
        now = time.monotonic()
        with self.lock:
            while self.global_attempts and self.global_attempts[0] < now - 60:
                self.global_attempts.popleft()
            if address not in self.attempts:
                if len(self.attempts) >= 2048:
                    self.attempts.popitem(last=False)
                self.attempts[address] = deque()
            bucket = self.attempts[address]
            while bucket and bucket[0] < now - 60:
                bucket.popleft()
            if len(bucket) >= 10 or len(self.global_attempts) >= 60:
                return False
            bucket.append(now)
            self.global_attempts.append(now)
            return True

    def verify(self, username: str, password: str) -> bool:
        # Always hash, including a wrong username, to avoid a username timing oracle.
        digest = password_hash(password, self.salt)
        return hmac.compare_digest(digest, self.digest) & hmac.compare_digest(
            username.encode(), self.username.encode()
        )

    def issue(self, scope: str = "") -> tuple[str, str]:
        token, csrf = secrets.token_urlsafe(32), secrets.token_urlsafe(32)
        with self.lock:
            now = time.monotonic()
            self.sessions = OrderedDict(
                (key, value) for key, value in self.sessions.items() if value[0] > now
            )
            if len(self.sessions) >= 1024:
                self.sessions.popitem(last=False)
            self.sessions[hashlib.sha256(token.encode()).hexdigest()] = (now + self.ttl, csrf, scope)
        return token, csrf

    def lookup(self, token: str | None, scope: str = "") -> str | None:
        if not token or len(token) > 128:
            return None
        key = hashlib.sha256(token.encode()).hexdigest()
        with self.lock:
            item = self.sessions.get(key)
            if not item:
                return None
            if item[0] <= time.monotonic():
                self.sessions.pop(key, None)
                return None
            return item[1] if item[2] == scope else None

    def revoke(self, token: str | None) -> None:
        if token:
            with self.lock:
                self.sessions.pop(hashlib.sha256(token.encode()).hexdigest(), None)
