"""The main window."""

{{#if toolkit == PySide6}}
import sys

from PySide6.QtWidgets import (
    QApplication,
    QLabel,
    QMainWindow,
    QPushButton,
    QVBoxLayout,
    QWidget,
)


class MainWindow(QMainWindow):
    def __init__(self) -> None:
        super().__init__()
        self.setWindowTitle("{{name}}")
        self.count = 0
        self.label = QLabel("Not clicked yet")
        button = QPushButton("Click me")
        button.clicked.connect(self.clicked)
        layout = QVBoxLayout()
        layout.addWidget(self.label)
        layout.addWidget(button)
        body = QWidget()
        body.setLayout(layout)
        self.setCentralWidget(body)

    def clicked(self) -> None:
        self.count += 1
        self.label.setText(f"Clicked {self.count} times")


def main() -> None:
    app = QApplication(sys.argv)
    window = MainWindow()
    window.resize(360, 160)
    window.show()
    sys.exit(app.exec())
{{else}}
import tkinter as tk
from tkinter import ttk


class MainWindow(ttk.Frame):
    def __init__(self, root: tk.Tk) -> None:
        super().__init__(root, padding=16)
        root.title("{{name}}")
        self.count = 0
        self.label = ttk.Label(self, text="Not clicked yet")
        self.label.pack(pady=(0, 8))
        ttk.Button(self, text="Click me", command=self.clicked).pack()
        self.pack(fill="both", expand=True)

    def clicked(self) -> None:
        self.count += 1
        self.label.config(text=f"Clicked {self.count} times")


def main() -> None:
    root = tk.Tk()
    MainWindow(root)
    root.mainloop()
{{/if}}
