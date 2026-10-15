import { Secret as Opaque, makeSecret } from secret;
import * as API from secret;

function read(value: Opaque) returns (word) {
    return value.value;
}

function main() returns (word) {
    return API.makeSecret().value;
}
