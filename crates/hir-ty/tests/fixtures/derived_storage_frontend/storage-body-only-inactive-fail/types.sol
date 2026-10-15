import * from std;
import * from std.Generic;

export { Box(*) };

// StorageGeneric is deliberately not visible in this defining module.
enum Box<a> {
    Box(a)
}
