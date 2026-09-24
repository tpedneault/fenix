import pytest

from {{name_snake}}.io import load

{{#if formats == csv}}

def test_a_csv_loads(tmp_path):
    path = tmp_path / "table.csv"
    path.write_text("a,b\n1,2\n3,4\n")
    assert len(load(path)) == 2

{{/if}}

def test_an_unknown_file_type_is_refused(tmp_path):
    path = tmp_path / "table.xyz"
    path.write_text("")
    with pytest.raises(ValueError):
        load(path)
