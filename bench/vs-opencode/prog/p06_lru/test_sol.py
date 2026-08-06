from sol import LRUCache
def test_basic():
    c = LRUCache(2); c.put(1,1); c.put(2,2)
    assert c.get(1) == 1
    c.put(3,3)
    assert c.get(2) == -1
    assert c.get(3) == 3
def test_missing(): assert LRUCache(1).get(9) == -1
def test_overwrite():
    c = LRUCache(2); c.put(1,1); c.put(1,5)
    assert c.get(1) == 5
def test_eviction_order():
    c = LRUCache(2); c.put(1,1); c.put(2,2); c.get(1); c.put(3,3)
    assert c.get(2) == -1 and c.get(1) == 1
