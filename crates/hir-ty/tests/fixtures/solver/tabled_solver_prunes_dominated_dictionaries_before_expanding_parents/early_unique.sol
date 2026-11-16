pragma solcore;

// ambiguous-leaf: false
import * from std;

trait Ready<a> {}
trait Middle<a> {}
trait Seed<a> {}
default impl<a> C<a> {}
impl<a> Ready<a> where a: Middle {}
impl<a> Middle<a> where a: Seed {}
impl<a> Seed<a> {}
trait D<a> {}
enum Box<a> {
    Box(a)
}
impl D<word> {}
impl<a> D<Box<a>> where a: D, a: C {}
