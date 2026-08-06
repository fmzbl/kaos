from sol import to_roman
def test_one(): assert to_roman(1) == "I"
def test_four(): assert to_roman(4) == "IV"
def test_nine(): assert to_roman(9) == "IX"
def test_fourteen(): assert to_roman(14) == "XIV"
def test_forty(): assert to_roman(40) == "XL"
def test_1994(): assert to_roman(1994) == "MCMXCIV"
def test_3999(): assert to_roman(3999) == "MMMCMXCIX"
