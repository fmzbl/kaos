def is_balanced(s):
    """True when every bracket in s is closed by the matching kind, in order."""
    pairs = {")": "(", "]": "[", "}": "{"}
    stack = []
    for ch in s:
        if ch in "([{":
            stack.append(ch)
        elif ch in pairs:
            # BUG: pops without checking the kind matches, and ignores underflow
            stack.pop()
    return len(stack) == 0
