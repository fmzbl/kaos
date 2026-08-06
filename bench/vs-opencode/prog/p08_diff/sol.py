def common_prefix(words):
    """The longest string that starts every word in the list."""
    if not words:
        return ""
    # BUG: assumes the first word is the shortest, and never stops early
    out = ""
    for i in range(len(words[0])):
        ch = words[0][i]
        for w in words:
            if w[i] != ch:
                return out
        out += ch
    return out
