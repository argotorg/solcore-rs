pragma solcore;

import * from std;
import * from std.dispatch;

// These literals intentionally contain raw CR characters. Together with an
// escaped line feed they also exercise CRLF values after concatenation.
contract StringCrLfRoundtrip {
    // #[() -> 4]
    function carriageReturn() public returns (uint256) {
        return uint256(strlenLit(concatLit("ab", "!")));
    }

    // #[() -> 19]
    function carriageReturnLineFeed() public returns (uint256) {
        return uint256(strlenLit(concatLit("line one\nline two", "!")));
    }
}
