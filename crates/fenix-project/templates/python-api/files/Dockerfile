FROM ghcr.io/astral-sh/uv:python{{python}}-bookworm-slim
WORKDIR /app
COPY pyproject.toml uv.lock ./
RUN uv sync --locked --no-install-project --no-dev
COPY . .
RUN uv sync --locked --no-dev
EXPOSE 8000
CMD ["uv", "run", "fastapi", "run", "src/{{name_snake}}/main.py", "--port", "8000"]
