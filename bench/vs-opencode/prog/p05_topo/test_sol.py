from sol import topo_sort
def test_line(): assert topo_sort(["a","b","c"], [("a","b"),("b","c")]) == ["a","b","c"]
def test_tie(): assert topo_sort(["b","a"], []) == ["a","b"]
def test_cycle(): assert topo_sort(["a","b"], [("a","b"),("b","a")]) is None
def test_diamond(): assert topo_sort(["a","b","c","d"], [("a","b"),("a","c"),("b","d"),("c","d")]) == ["a","b","c","d"]
def test_empty(): assert topo_sort([], []) == []
