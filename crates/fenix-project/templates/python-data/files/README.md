# {{name}}

Data analysis with {{frames}}, managed by [uv](https://docs.astral.sh/uv/).

- `data/raw/` -- the data as it came; not committed.
- `data/processed/` -- what the code makes from it; not committed.
- `src/{{name_snake}}/io.py` -- reading and writing tables.
- `src/{{name_snake}}/analysis.py` -- a first look: `uv run python -m {{name_snake}}.analysis`.
{{#if notebooks}}
- `notebooks/` -- `uv run jupyter lab`.
{{/if}}
