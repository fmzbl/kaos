from sol import merge
def test_basic(): assert merge([[1,3],[2,6],[8,10],[15,18]]) == [[1,6],[8,10],[15,18]]
def test_touching(): assert merge([[1,4],[4,5]]) == [[1,5]]
def test_empty(): assert merge([]) == []
def test_unsorted(): assert merge([[5,6],[1,2]]) == [[1,2],[5,6]]
def test_contained(): assert merge([[1,10],[2,3]]) == [[1,10]]
