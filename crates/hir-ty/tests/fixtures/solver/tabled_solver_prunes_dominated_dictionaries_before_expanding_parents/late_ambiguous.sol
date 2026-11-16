pragma solcore;

// ambiguous-leaf: true
import * from std;

trait Ready<a> {}
trait Middle<a> {}
trait Seed<a> {}
trait Leaf<a> {}
default impl<a> C<a> where a: Ready {}
impl<a> Ready<a> where a: Middle {}
impl<a> Middle<a> where a: Seed {}
impl<a> Seed<a> {}
default impl<a> C<a> where a: Leaf {}
impl Leaf<word> {}
trait D<a> {}
enum Box<a> {
    Box(a)
}
impl D<word> {}
impl<a> D<Box<a>> where a: D, a: C {}
