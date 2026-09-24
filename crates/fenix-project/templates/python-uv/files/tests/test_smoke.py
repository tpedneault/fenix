from pathlib import Path


def test_the_project_has_a_pyproject():
    assert (Path(__file__).parent.parent / "pyproject.toml").is_file()
