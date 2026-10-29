pragma solcore;

// Keep the callable function at module scope so this frontend fixture does not
// generate contract dispatch.
import * as api from traits;

#[derive(api.Marker)]
enum Tagged {}

function run(value: word) returns (word) {
    return value;
}
