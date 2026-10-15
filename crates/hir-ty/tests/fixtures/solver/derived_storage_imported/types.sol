import * from storage_support;

export { Box(*) };

enum Box<a> {
    Box(a)
}
