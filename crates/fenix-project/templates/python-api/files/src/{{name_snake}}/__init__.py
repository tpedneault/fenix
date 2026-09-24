"""{{name}}."""


def main() -> None:
    import uvicorn

    uvicorn.run("{{name_snake}}.main:app", host="127.0.0.1", port=8000)
