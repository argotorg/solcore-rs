import * from std;
import * from std.Generic;
import * from std.StorageGeneric;

export { Wrapper(*) };

enum Wrapper {
    Wrapper(mapping(uint256 => uint256))
}
