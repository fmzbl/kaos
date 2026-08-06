from sol import parse_query
def test_simple(): assert parse_query("a=1&b=2") == {"a":"1","b":"2"}
def test_leading_q(): assert parse_query("?a=1") == {"a":"1"}
def test_flag(): assert parse_query("a") == {"a":""}
def test_repeat(): assert parse_query("a=1&a=2") == {"a":["1","2"]}
def test_empty(): assert parse_query("") == {}
