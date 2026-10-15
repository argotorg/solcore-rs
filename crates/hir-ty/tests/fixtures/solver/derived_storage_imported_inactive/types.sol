import { Generic } from generic;

export { Box(*) };

// Generic is visible here, but StorageDeriving deliberately is not.
enum Box<a> {
    Box(a)
}
