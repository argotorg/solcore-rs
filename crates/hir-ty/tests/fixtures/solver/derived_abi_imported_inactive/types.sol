import { Generic } from generic;

export { Box(*) };

// Generic is visible here, but the ABIDeriving marker deliberately is not.
enum Box<a> {
    Box(a)
}
