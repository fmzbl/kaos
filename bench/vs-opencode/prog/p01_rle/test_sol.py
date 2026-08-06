from sol import encode
def test_basic(): assert encode("aaabbc") == "a3b2c1"
def test_empty(): assert encode("") == ""
def test_single(): assert encode("z") == "z1"
def test_alternating(): assert encode("ababab") == "a1b1a1b1a1b1"
def test_long_run(): assert encode("x"*12) == "x12"
