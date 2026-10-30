pragma solcore;

import { storage, array, string, CanStore } from std;

function rebind(lhs: storage<array<string>>, rhs: storage<array<string>>) {
    lhs = rhs;
    return ();
}
