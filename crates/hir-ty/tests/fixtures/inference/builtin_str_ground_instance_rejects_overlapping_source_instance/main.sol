pragma solcore;

import * from std;

impl Str<string> {
    function fromString(comptime value: string) returns (string) {
        return value;
    }
}

impl Str<memory<string>> {
    function fromString(comptime value: string) returns (memory<string>) {
        return Str.fromString(value);
    }
}
