// Adapted from upstream test/new-syntax/derive: keep the callable function
// at module scope so this frontend fixture does not generate contract dispatch.
import traits;

#[derive(traits.Marker)]
enum Tagged {}

function run(value: word) returns (word) {
    return value;
}
