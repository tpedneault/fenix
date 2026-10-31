from fastapi.testclient import TestClient

from {{name_snake}}.main import app

client = TestClient(app)


def test_health():
    assert client.get("/health").json() == {"status": "ok"}


def test_an_item_can_be_created_and_read_back():
    created = client.post("/items", json={"name": "bolt", "quantity": 4})
    assert created.status_code == 201
    item = client.get(f"/items/{created.json()['id']}").json()
    assert item == {"name": "bolt", "quantity": 4}


def test_a_missing_item_is_404():
    assert client.get("/items/999").status_code == 404
