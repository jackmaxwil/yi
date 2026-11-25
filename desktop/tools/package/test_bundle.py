#!/usr/bin/env python3
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))

import bundle
import package
import receipts


def check():
    try:
        assert package.BUNDLE is bundle.BUNDLE
        assert package.ID is bundle.ID
        assert receipts.BUNDLE is bundle.BUNDLE
        assert receipts.EXE is bundle.EXE
        assert receipts.ID is bundle.ID
        assert receipts.APP == bundle.BUNDLE / "Contents/MacOS" / bundle.EXE
    except (AssertionError, AttributeError) as error:
        print(f"not ok 1 - R1 package tools share bundle constants: {error}")
        return 1
    print("ok 1 - R1 package tools share bundle constants")
    return 0


if __name__ == "__main__":
    sys.exit(check())
