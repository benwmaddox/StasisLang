# Third-party notices

Windows Stasis toolchain archives include `clang-cl.exe` from the LLVM Project so the generated
game runtime bridge can be compiled without a separate toolchain installation. LLVM is licensed
under the Apache License v2.0 with LLVM Exceptions. Windows archives include the applicable
license, exception, copyright, and third-party notice text in `RUST-LLVM-COPYRIGHT.html` and
`LLVM-THIRD-PARTY-NOTICES.txt`; the upstream license is also published at
<https://llvm.org/LICENSE.txt>.

The archive also includes `lld-link.exe` from the Rust toolchain distribution. LLVM and Rust
license notices remain applicable to those binaries.

The graphics runtime includes `minimp3` at commit
`ea99364f61c14656440e8d77e9c233ccf3124633` to decode packaged MP3 audio into bounded host memory.
The project is dedicated to the public domain under CC0 1.0; the complete notice is retained in
`runtime/MINIMP3-LICENSE.txt`.
