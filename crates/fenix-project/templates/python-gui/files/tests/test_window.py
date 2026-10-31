from {{name_snake}} import window


def test_the_window_module_has_an_entry_point():
    assert callable(window.main)
