import * from std;
import * from std.Generic;
import * from std.StorageGeneric;
import { Inner } from api;

export { Outer(*) };

enum Outer {
    Outer(Inner)
}
