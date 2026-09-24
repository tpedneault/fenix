{{#if framework == typer}}
from typer.testing import CliRunner

from {{name_snake}}.cli import app


def test_it_says_hello():
    result = CliRunner().invoke(app, ["Ada"])
    assert result.exit_code == 0
    assert "Hello, Ada!" in result.output
{{/if}}
{{#if framework == click}}
from click.testing import CliRunner

from {{name_snake}}.cli import cli


def test_it_says_hello():
    result = CliRunner().invoke(cli, ["Ada"])
    assert result.exit_code == 0
    assert "Hello, Ada!" in result.output
{{/if}}
{{#if framework == argparse}}
from {{name_snake}}.cli import main


def test_it_says_hello(capsys):
    main(["Ada"])
    assert "Hello, Ada!" in capsys.readouterr().out
{{/if}}
