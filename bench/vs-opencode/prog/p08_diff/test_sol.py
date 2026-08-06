from sol import common_prefix
def test_basic(): assert common_prefix(["flower","flow","flight"]) == "fl"
def test_none(): assert common_prefix(["dog","car"]) == ""
def test_empty_list(): assert common_prefix([]) == ""
def test_shorter_later(): assert common_prefix(["abcd","ab"]) == "ab"
def test_identical(): assert common_prefix(["same","same"]) == "same"
def test_one(): assert common_prefix(["solo"]) == "solo"
