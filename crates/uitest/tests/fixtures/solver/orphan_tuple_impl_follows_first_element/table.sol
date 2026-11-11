pragma solcore;

export { Get };

// Like std's `RValueIdxAccess`, the trait takes its two input types as one
// tuple.
trait Get<col_idx, val> {
    function get(ci: col_idx) returns (val);
}
