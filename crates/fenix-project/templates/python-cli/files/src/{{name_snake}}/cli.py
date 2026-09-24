"""The command line."""

{{#if framework == typer}}
import typer

app = typer.Typer(help="{{name}}")


@app.command()
def hello(name: str = typer.Argument("world"), shout: bool = False) -> None:
    """Say hello to NAME."""
    text = f"Hello, {name}!"
    typer.echo(text.upper() if shout else text)


def main() -> None:
    app()
{{/if}}
{{#if framework == click}}
import click


@click.command()
@click.argument("name", default="world")
@click.option("--shout", is_flag=True, help="Say it louder.")
def cli(name: str, shout: bool) -> None:
    """Say hello to NAME."""
    text = f"Hello, {name}!"
    click.echo(text.upper() if shout else text)


def main() -> None:
    cli()
{{/if}}
{{#if framework == argparse}}
import argparse


def main(argv: list[str] | None = None) -> None:
    parser = argparse.ArgumentParser(prog="{{name_kebab}}", description="Say hello to NAME.")
    parser.add_argument("name", nargs="?", default="world")
    parser.add_argument("--shout", action="store_true", help="say it louder")
    args = parser.parse_args(argv)
    text = f"Hello, {args.name}!"
    print(text.upper() if args.shout else text)
{{/if}}
