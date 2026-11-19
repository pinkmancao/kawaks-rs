# Third-party references

The Kabuki transform and CPS1 QSound keys follow [MAME's Kabuki implementation](https://github.com/mamedev/mame/blob/master/src/mame/capcom/kabuki.cpp).

The CPS1 renderer uses the board configuration, tile layout, and mixing rules documented by MAME in [CPS1 video](https://github.com/mamedev/mame/blob/master/src/mame/capcom/cps1_v.cpp) and [the CPS1 driver](https://github.com/mamedev/mame/blob/master/src/mame/capcom/cps1.cpp).

Copyright (c) Nicola Salmoria. All rights reserved.
Copyright (c) Paul Leaman. All rights reserved.

Redistribution and use in source and binary forms, with or without modification,
are permitted provided that the following conditions are met:

1. Redistributions of source code must retain the above copyright notice, this
   list of conditions and the following disclaimer.
2. Redistributions in binary form must reproduce the above copyright notice,
   this list of conditions and the following disclaimer in the documentation
   and/or other materials provided with the distribution.
3. Neither the name of the copyright holder nor the names of its contributors
   may be used to endorse or promote products derived from this software without
   specific prior written permission.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS"
AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE
IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE LIABLE FOR
ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES
(INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES;
LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON
ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT
(INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE OF THIS
SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.

CPU dependencies are r68k (MIT) and z80emu (LGPL-3.0-or-later). Review crate
licenses and distribution obligations before shipping binaries.

The optional window frontend uses [rust-sdl2](https://github.com/Rust-SDL2/rust-sdl2) (MIT) and bundles [SDL2](https://github.com/libsdl-org/SDL/tree/SDL2) (zlib). The headless build has no SDL dependency.

The CPS2 Feistel tables in [cps2_tables.rs](src/cps2_tables.rs) are adapted
from [MAME cps2crypt.cpp](https://github.com/mamedev/mame/blob/master/src/mame/capcom/cps2crypt.cpp)
by Paul Leaman, Andreas Naive, Nicola Salmoria and Charles MacDonald under the
BSD 3-Clause license reproduced above.

Neo Geo uses [emu198x-yamaha-ym2610](https://crates.io/crates/emu198x-yamaha-ym2610)
under GPL-2.0-or-later. Its YM2612 and AY-3-8910 dependencies and their licenses
are recorded in the upstream crates. Neo Geo hardware behavior was cross-checked
with [MAME's Neo Geo driver](https://github.com/mamedev/mame/blob/master/src/mame/snk/neogeo.cpp)
and [uPD4990A model](https://github.com/mamedev/mame/blob/master/src/devices/machine/upd1990a.cpp).
