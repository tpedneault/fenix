"""The service."""

from fastapi import FastAPI, HTTPException
from pydantic import BaseModel

app = FastAPI(title="{{name}}")


class Item(BaseModel):
    name: str
    quantity: int = 1


items: dict[int, Item] = {}


@app.get("/health")
def health() -> dict[str, str]:
    return {"status": "ok"}


@app.post("/items", status_code=201)
def create_item(item: Item) -> dict[str, int]:
    item_id = len(items) + 1
    items[item_id] = item
    return {"id": item_id}


@app.get("/items/{item_id}")
def read_item(item_id: int) -> Item:
    if item_id not in items:
        raise HTTPException(status_code=404, detail="no such item")
    return items[item_id]
