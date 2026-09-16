// A constant-product pool behind an opaque type: Pool is exported without
// its constructor, so a Pool can only be created or changed through the
// functions in this module.
import * from std;
import * from std.Generic;
import * from std.StorageGeneric;

export { Pool, mkPool, reserveX, reserveY, swapXforY, addLiquidity };

enum Pool { Pool(uint256, uint256) }

// The single point where the nonzero-reserve invariant is established.
function mkPool(x: uint256, y: uint256) returns (Pool) {
    require(x > uint256(0) && y > uint256(0), "empty reserves");
    return Pool(x, y);
}

function reserveX(p: Pool) returns (uint256) {
    match (p) { case Pool(x, _) { return x; } }
}

function reserveY(p: Pool) returns (uint256) {
    match (p) { case Pool(_, y) { return y; } }
}

// A swap may never decrease the product of the reserves. The rounding
// against the trader makes the property hold; the assertion enforces it
// against future changes to the arithmetic.
function swapXforY(p: Pool, dx: uint256) returns ((Pool, uint256)) {
    match (p) {
        case Pool(x, y) {
            let k = x * y;
            let nx = x + dx;
            let ny = (k + nx - uint256(1)) / nx;
            let dy = y - ny;
            require(nx * ny >= k, "product decreased");
            return (mkPool(nx, ny), dy);
        }
    }
}

// LP share accounting is omitted.
function addLiquidity(p: Pool, dx: uint256, dy: uint256) returns (Pool) {
    match (p) {
        case Pool(x, y) {
            return mkPool(x + dx, y + dy);
        }
    }
}
