// Keep the callable function at module scope so this frontend fixture does not
// generate contract dispatch.
import { Generic, CloneLike, Marker as Tagged } from traits;

#[derive(CloneLike, Tagged)]
enum Box {
    Box(word)
}

function run(value: Box) returns (Box) {
    return CloneLike.clone(value);
}
