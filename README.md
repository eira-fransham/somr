# SOMR, Single-Owner, Multiple-Reader pointer

Essentially, `Rc` without the refcounting. This can be useful for a cache where
you want to ensure that you can't leak memory by having some upgraded pointers
lying around. There is currently no atomic, thread-safe version of this (I don't
trust myself with the single-threaded one, let alone the multithreaded one).

Also, it shrinks the reserved allocation when the owner is dropped or unwrapped,
so `Somr<SomeReallyBigStruct>` doesn't leave a bunch of memory allocated but
unused when there are still weak pointers hanging around (like `Rc` does). One
downside is that we can only have `2 << 30` weak pointers on 32-bit hosts and
`2 << 62` weak pointers on 64-bit hosts. This isn't too much of a restriction,
but it is significantly less than `Rc`.

### WARNING: THIS IS NOT PRODUCTION-READY

This crate is something on the order of 300% unsafe code and it has not been
audited by anyone other than me. I've had a bunch of segfaults in it that I have
since fixed, but it's entirely possible that there are edge-cases that I haven't
accounted for. Use at your own peril, or (even better) don't use at all.
