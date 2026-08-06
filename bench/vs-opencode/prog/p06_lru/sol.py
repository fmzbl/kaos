class LRUCache:
    """Fixed-capacity cache. get(k) returns the value or -1; put(k, v) inserts,
    evicting the least recently used entry when full. Both count as a use."""
    def __init__(self, capacity):
        raise NotImplementedError
    def get(self, key):
        raise NotImplementedError
    def put(self, key, value):
        raise NotImplementedError
