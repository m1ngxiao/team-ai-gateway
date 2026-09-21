FROM python:3.12.14-slim-bookworm
ENV PYTHONDONTWRITEBYTECODE=1 PYTHONUNBUFFERED=1
WORKDIR /app
COPY requirements.txt ./
RUN pip install --no-cache-dir -r requirements.txt
COPY dashboard ./dashboard
USER 10001:10001
EXPOSE 48763
CMD ["python", "-m", "uvicorn", "dashboard.server:create_app", "--factory", "--host", "0.0.0.0", "--port", "48763", "--workers", "1", "--no-proxy-headers", "--no-access-log", "--limit-concurrency", "64", "--timeout-keep-alive", "5"]
