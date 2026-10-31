"""Reading and writing the project's tables."""

from pathlib import Path

{{#if frames == pandas}}
import pandas as pd

Frame = pd.DataFrame
{{else}}
import polars as pl

Frame = pl.DataFrame
{{/if}}

DATA = Path(__file__).resolve().parents[2] / "data"
RAW = DATA / "raw"
PROCESSED = DATA / "processed"


def load(path: str | Path) -> Frame:
    """Reads a table, by its file type."""
    path = Path(path)
    suffix = path.suffix.lower()
{{#if formats == csv}}
    if suffix == ".csv":
{{#if frames == pandas}}
        return pd.read_csv(path)
{{else}}
        return pl.read_csv(path)
{{/if}}
{{/if}}
{{#if formats == parquet}}
    if suffix == ".parquet":
{{#if frames == pandas}}
        return pd.read_parquet(path)
{{else}}
        return pl.read_parquet(path)
{{/if}}
{{/if}}
{{#if formats == excel}}
    if suffix in (".xlsx", ".xls"):
{{#if frames == pandas}}
        return pd.read_excel(path)
{{else}}
        return pl.read_excel(path)
{{/if}}
{{/if}}
    raise ValueError(f"don't know how to read {path.name}")


def save(frame: Frame, name: str) -> Path:
    """Writes `frame` to data/processed and says where."""
    PROCESSED.mkdir(parents=True, exist_ok=True)
{{#if formats == parquet}}
    path = PROCESSED / f"{name}.parquet"
{{#if frames == pandas}}
    frame.to_parquet(path)
{{else}}
    frame.write_parquet(path)
{{/if}}
{{else}}
    path = PROCESSED / f"{name}.csv"
{{#if frames == pandas}}
    frame.to_csv(path, index=False)
{{else}}
    frame.write_csv(path)
{{/if}}
{{/if}}
    return path
