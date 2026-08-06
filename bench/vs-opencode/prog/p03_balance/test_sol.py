from sol import is_balanced
def test_ok(): assert is_balanced("([]{})") is True
def test_mismatch(): assert is_balanced("([)]") is False
def test_underflow(): assert is_balanced(")(") is False
def test_empty(): assert is_balanced("") is True
def test_unclosed(): assert is_balanced("(((") is False
def test_text(): assert is_balanced("a(b[c]d)e") is True
