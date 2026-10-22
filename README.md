<div align="center">
  <h1>benilla</h1>
  <p><b>A complete World of Warcraft client, written from scratch in Rust and <a href="https://bevy.org">Bevy</a></b></p>
  <p>
    <a href="https://discord.gg/wJSJx467G4"><img src="https://img.shields.io/discord/1529280129518538922?style=for-the-badge&logo=discord&logoColor=white&label=discord&color=5865F2" alt="Discord"></a>
    <a href="https://www.youtube.com/playlist?list=PLdCnpZNKxyb8"><img src="https://img.shields.io/badge/devlog-youtube-FF0000?style=for-the-badge&logo=youtube&logoColor=white" alt="YouTube devlog"></a>
    <a href="LICENSE-MIT"><img src="https://img.shields.io/badge/license-MIT%20%2F%20Apache--2.0-blue?style=for-the-badge" alt="License"></a>
  </p>
  <p>
    <a href="#this-fork"><img src="https://img.shields.io/badge/fork-3.3.5%20%7C%20Wrath-of-the-Lich-King-7a3ce8?style=for-the-badge" alt="The 3.3.5 fork"></a>
  </p>
</div

benilla is the [upstream project](https://github.com/samwhosung/benilla): a complete World of
Warcraft 1.12.1 client. It plays the whole game — character creation, questing and professions,
dungeons and raids, battlegrounds and honor, groups, guilds, trade, mail and the auction house,
on the stock interface and with your addons — connecting to a 1.12.1 server over the original
protocol and reading the game's data from your own install. Every file format, the network
protocol and the interface engine are written from scratch, with no original client code, no
third-party WoW crates and no bundled game assets.

**This is the 3.3.5 fork** — where the Wrath of the Lich King work lives. Upstream takes only
changes that make the client more like 1.12.1 (its `docs/CONTRIBUTING.md` says so, and its
maintainer pointed 3.3.5 work here); so the fork carries the Wrath work in a `3.3.5` branch on
top of a `main` that tracks upstream, the way Trinity Core carries a version line on top of
trunk ([Branches](#branches)). One feature per branch, verified against real 3.3.5 client files
before it lands.

## This fork

Two features are in, both on top of current upstream `main`:

**The Wrath model reader.** A new version-aware `M2Layout` in `benilla-m2` carries every
version-dependent offset and stride for builds 256–264 in one place. Wrath (264) files are read
through it: the nested per-sequence `M2Track` (each sequence its own key clip, rebased by band
start), the 36-byte event record with its value-less timer track, the `0x1dc` particle record
with keyed `u16`-timestamp over-life, the `0xb0` ribbon record, and the external `SKIN` geometry
file the profile no longer embeds. Pre-Wrath (256–263) keeps the reference layout
byte-for-byte. Byte-verified against a real Wrath file — the Icecrown raid axe
(`Axe_2H_IcecrownRaid_D_01.M2` + its `_0100.skin`): five skin batches, stand
sequence/skeleton/bounds, the `$WTT`/`$WTB` events, the emitter's over-life tracks, all asserted
bit-exact.

**The `DBFilesClient` DBC source, behind a swappable seam.** 3.x clients read their DBC tables
from a `DBFilesClient` sqlite file instead of loose WDBC files. This fork's `Chain` adopts a
`DbcProvider` at open when a `Data` folder ships that file (a single-MPQ chain or an absent file
has none), and `crate::dbc::load` — the facade every table loader uses — asks the provider first
and reads the patch chain's WDBC file when it does not carry the table or its layout does not
match the loader's schema. Everything sqlite stays in `benilla-formats/src/dbfiles.rs`: the
layout probe, the column decode, the string slots; the ~90 loaders see only `RecordSets`. A
different source later — another database, a test fixture — is a new `DbcProvider` implementation
adopted in the one place that selects it. The decode is verified table-for-table against the
patch chain's files: 154 tables in a real 3.3.5 file round-trip with one mismatch, the known
data-state difference in ItemDisplayInfo (29604 vs 29605), not a decode difference.

An install with no `DBFilesClient` behaves exactly like upstream: 1.12.1 is untouched by this
fork by default.
Overrides may intentionally change rows or strings. The codec round-trip test exports the
patch chain's bytes into a temporary database before comparing every decoded field, rather
than assuming an installed override is identical to its source.

## Branches

The fork keeps the upstream line alive, so the 1.12.1 improvements keep flowing in — the shape
Trinity Core gives a version line:

- **`main`** mirrors upstream `main`, the 1.12.1 line. It is a clean tracking branch: fetch the
  upstream, advance `main`, and nothing fork-specific lives on it.
- **`3.3.5`** is this line: the two features above, on current `main`. Build from here.

Integrating upstream is a merge: `git fetch upstream`, advance `main`, then `git merge main`
into `3.3.5` and resolve the overlap — the docs regenerate with `scripts/genmap.sh`, and
`scripts/check.sh` decides. A new 3.3.5 feature lands the same way: its own branch, verified
against real files, merged into `3.3.5`.

Upstream is [samwhosung/benilla](https://github.com/samwhosung/benilla). A clone made from the
fork link carries it as `upstream` already; otherwise
`git remote add upstream https://github.com/samwhosung/benilla`.

## Status

- **1.12.1:** complete and fully playable, exactly as upstream — see
  [the upstream README](https://github.com/samwhosung/benilla) for the whole inventory.
- **3.3.5, what is in:** the Wrath M2 model reader (verified against real files, above) and the
  `DBFilesClient` DBC source (verified against a real 3.3.5 file, above).
- **3.3.5, what is not:** a Wrath connection. The wire protocol is still 1.12.1's, and the map
  and interface stacks have not been walked against 3.3.5 data. That is the work below.

## Jump in here

The fork is a list of known unknowns, in roughly the order a Wrath client needs them:

1. **The 3.3.5 wire protocol** — the critical path. `benilla-protocol` and `benilla-srp` speak
   5875; a 3.3.5 server speaks build 264. The reverse-engineering notes upstream builds against
   ([benilla-re](https://github.com/samwhosung/benilla-re)) are the 1.12.1 client's addresses;
   the 3.3.5 client's are the new corpus. Auth, opcodes, the object mirror, the delta encoding.
2. **The map stack at 3.3.5** — verify the ADT/WDT readers against Wrath files (both are v131
   through 3.3.5, so this may be close), and extend the WMO reader to the Wrath version. The M2
   side of it is done.
3. **The DBC seam, wider** — more `DbcProvider` coverage (the loaders that hand-parse WDBC bytes
   today), a second implementation, or the test fixture a provider needs to be tested without a
   3.3.5 install. The seam is in `benilla-formats/src/dbfiles.rs`.
4. **The 3.3.5 tables and UI** — where Wrath's FrameXML and table set diverge from 1.12's, the
   UI engine and the loaders follow.
5. **Keeping the fork honest** — merge `main` into `3.3.5` as upstream moves (both features
   already sit on current `main`), and keep one feature per branch with the real-file
   verification that lands it.

[`docs/MAP.md`](docs/MAP.md) maps every crate and subsystem, [`docs/DEVIATIONS.md`](docs/DEVIATIONS.md)
lists everywhere benilla differs from the reference client, and
[`docs/CONTRIBUTING.md`](docs/CONTRIBUTING.md) says how a change is judged; both are generated
from the code.

## Running it

For a new collaborator checkout on Linux, install the native build tools, the pinned Rust
toolchain and compile **all workspace binaries** with:

```sh
./build-benilla.sh
WOW_DATA=/path/to/WoW/Data ./run-benilla.sh
```

The build script supports Debian/Ubuntu flavors, Nobara/Fedora and Arch derivatives, plus macOS with
Homebrew and Xcode command line tools already installed. It may ask for `sudo` for system
packages; Rust and Cargo downloads run as your user. It does not download game data, a server
or GPU drivers. `./build-benilla.sh --help` lists options, including `--jobs 2` for smaller
machines and `--skip-system --skip-toolchain` for an already configured environment.
Builds use the committed lockfile and include the client, world viewer, extractors and
`dbfiles` tool. Tests and gates are separate; see
[`docs/CONTRIBUTING.md`](docs/CONTRIBUTING.md#setting-up).

[`run-benilla.sh`](run-benilla.sh) accepts optional user/password arguments or the existing
`WOW_USER`/`WOW_PASS` environment variables; with neither, use the login screen.
`WOW_HOST` defaults to `127.0.0.1:3724`. If `WOW_DATA` is unset, a sibling
`VanillaClient/Data` directory is used when present; otherwise the normal install resolver
applies. An explicitly empty `WOW_DATA` still means no install. The launcher runs Cargo so
changed code is rebuilt, including when `CARGO_TARGET_DIR` points outside the checkout.

The 1.12.1 client runs as upstream — a 1.12.1 install (build 5875), a 1.12.1 server with Warden
off ([vmangos](https://github.com/vmangos/core) is what development runs against), stable Rust
and a C compiler:

```sh
WOW_DATA=/path/to/WoW/Data cargo run --release -p benilla
```

The 3.3.5 DBC source is opt-in by presence: drop a `DBFilesClient` sqlite file into the install's
`Data` folder and the chain adopts it at open, saying so —
`benilla: adopting …/DBFilesClient as a DBC source (154 tables)`. Tables that match a loader's
schema come from the file; everything else comes from the patch chain as before. Build a
`DBFilesClient` file from a folder of extracted 3.3.5 DBCs with the `dbfiles` bin:

```sh
cargo run -p benilla-formats --bin dbfiles -- /path/to/dbc-dir /path/to/DBFilesClient
```

The server, host, credentials and addon settings work exactly as upstream documents them.

Loose-file patch folders also mount in the data chain: `patch-a.MPQ/` or `patch-a.patch/`
contains files at their archive-relative paths, such as `DBFilesClient/AreaTable.dbc`.
They follow the same patch priority as the corresponding MPQ name. A real MPQ file wins
over a folder with the same mount name; folder indexing is deterministic. Game files remain
local to your install and must never be committed.

## Contributing

Pull requests are open on this fork, one feature per branch, against `3.3.5`. The bar is the one upstream sets in
[`docs/CONTRIBUTING.md`](docs/CONTRIBUTING.md): a change is read against the client's own
behaviour, and the 3.3.5 work here follows the same discipline — pinned to a real client file,
asserted bit-exact, with the verification in the pull request. Questions and ideas are welcome
on the [Discord](https://discord.gg/wJSJx467G4).

---

benilla is an independent fan project, not affiliated with or endorsed by Blizzard
Entertainment. It ships **no Blizzard content**: no art, models, sounds, maps, MPQ contents or
FrameXML, in either client version. You provide your own legally obtained client, and the
interface runs off its files at runtime. Early inspiration and file format guidance came from
the [wowemulation-dev](https://github.com/wowemulation-dev) community, and
[warcraft-rs](https://github.com/wowemulation-dev/warcraft-rs) in particular.

World of Warcraft is a trademark of Blizzard Entertainment, Inc. Our own code is licensed under
[MIT](LICENSE-MIT) or [Apache 2.0](LICENSE-APACHE), at your option, as upstream's is. The two
vendored components under `third_party/` keep their own upstream licenses, alongside each.
