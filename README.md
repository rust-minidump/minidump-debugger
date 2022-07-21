# minidump-debugger

An experimental GUI for [rust-minidump](https://github.com/rust-minidump/rust-minidump) based on [egui](https://www.egui.rs/).

# Functionality

At this point the UI is mostly at parity with [minidump-stackwalk](https://github.com/rust-minidump/rust-minidump/tree/main/minidump-stackwalk)

* raw minidump inspection (for debugging weird minidumps)
* stackwalking (via cfi, frame pointers, and scanning)
* symbolication (via symbol server, either using native binaries or breakpad .sym)
* processing the minidump's metadata
* trace logs for debugging the stackwalk

# Future Functionality?

* log searching/sorting/filtering based on tracing spans ("give me all the info on this thread")
* builtin hexdump viewing (we currently get some from the raw minidump printing, but it's very slow because it doesn't know where we're looking)
* surface more random pieces of information (crash time, endianess, ...)
* `Linux*` stream raw inspection (they have a weird format)
* surface recovered arguments (currently only computed in the x86 backend, kinda jank)
* steal some [socc-pair](https://github.com/Gankra/socc-pair/) features? (benching, fetching dumps, mocking symbol server, diffing)
* allow the dump to be pointed at a build dir to compute local symbols?

# Future Cleanups?

* properly expand table row-heights for line-wrapping items
* better pointer-sized-value formatting (pad 64-bit to 16 chars)
* make more text selectable (bare labels suck for most of what we display)
* don't make the `symbol cache` checkbox so terribly dangerous (will blindly delete the dir at that path, should just disable the cache)

# Screenshots

![Screenshot 2022-07-21 171657](https://user-images.githubusercontent.com/1136864/180317424-c6abb289-dc63-4aa7-a092-2e29b5fb88aa.png)

![Screenshot 2022-07-21 171806](https://user-images.githubusercontent.com/1136864/180317446-04be55aa-206c-4d84-b303-6bcbfb7068bc.png)

![Screenshot 2022-07-21 171851](https://user-images.githubusercontent.com/1136864/180317454-3f158dd7-47e2-455f-9847-e42e58a918a2.png)

![Screenshot 2022-07-21 171957](https://user-images.githubusercontent.com/1136864/180317467-bd2bfa4a-ecbb-4fcc-8b36-dfa807397e83.png)
