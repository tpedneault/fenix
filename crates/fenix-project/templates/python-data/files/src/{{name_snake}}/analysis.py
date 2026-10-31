"""A first look at the data: `uv run python -m {{name_snake}}.analysis`."""

{{#if plots == matplotlib}}
import matplotlib.pyplot as plt
{{/if}}
{{#if plots == plotly}}
import plotly.express as px
{{/if}}
{{#if frames == polars}}
import polars.selectors as cs
{{/if}}

from {{name_snake}}.io import RAW, Frame, load


def summarise(frame: Frame) -> Frame:
    """Count, mean, spread and range of each column."""
    return frame.describe()


{{#if plots == matplotlib}}
def plot(frame: Frame) -> None:
    """A histogram of each numeric column."""
{{#if frames == pandas}}
    frame.select_dtypes("number").hist()
{{else}}
    numeric = frame.select(cs.numeric())
    for name in numeric.columns:
        plt.hist(numeric[name].to_list(), alpha=0.5, label=name)
    plt.legend()
{{/if}}
    plt.show()


{{/if}}
{{#if plots == plotly}}
def plot(frame: Frame) -> None:
    """A histogram of each numeric column."""
{{#if frames == pandas}}
    px.histogram(frame.select_dtypes("number")).show()
{{else}}
    px.histogram(frame.select(cs.numeric())).show()
{{/if}}


{{/if}}
def main() -> None:
    files = sorted(p for p in RAW.iterdir() if p.is_file() and not p.name.startswith("."))
    if not files:
        print(f"Put a data file in {RAW} and run this again.")
        return
    frame = load(files[0])
    print(f"{files[0].name}: {len(frame)} rows")
    print(summarise(frame))
{{#if plots != none}}
    plot(frame)
{{/if}}


if __name__ == "__main__":
    main()
