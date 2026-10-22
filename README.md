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
</div>

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

The existing `3.3.5` branch already combines the Wrath model reader, optional SQLite DBC
source, loose patch folders, encrypted MPQ reading, and build/launcher tools. Do not reapply
the old split feature branches or create another integration branch to continue this work.

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

**The `DBFilesClient` DBC source, behind a swappable seam.** This fork accepts an optional
`Data/DBFilesClient` SQLite database as a DBC override; it is not a requirement of the stock
3.3.5 client. This fork's `Chain` adopts a
`DbcProvider` at open when a `Data` folder ships that file (a single-MPQ chain or an absent file
has none), and `crate::dbc::load` — the facade every table loader uses — asks the provider first
and reads the patch chain's WDBC file when it does not carry the table or its layout does not
match the loader's schema. Everything sqlite stays in `benilla-formats/src/dbfiles.rs`: the
layout probe, the column decode, the string slots; the ~90 loaders see only `RecordSets`. A
different source later — another database, a test fixture — is a new `DbcProvider` implementation
adopted in the one place that selects it. In the tested 154-table install, 152 tables
round-tripped field-for-field from the patch chain's bytes; two record layouts smaller than
their declared four-byte slots were refused, retaining the DBC-file fallback. The installed
ItemDisplayInfo override had 29605 rows versus the chain's 29604: an intentional data-state
difference must not be mistaken for a codec failure.

An install with no `DBFilesClient` behaves exactly like upstream: 1.12.1 is untouched by this
fork by default.
Overrides may intentionally change rows or strings. The codec round-trip test exports the
patch chain's bytes into a temporary database before comparing every decoded field, rather
than assuming an installed override is identical to its source.

**Archive inputs.** Packed MPQs and loose `patch-a.MPQ/` or `patch-a.patch/` folders share the
patch chain. The reader supports encrypted stored and compressed sectors, encrypted offset
tables, and filename keys adjusted with `FIX_KEY`, for whole-file reads and streaming seeks.
Single-unit and PTCH entries remain unsupported. A regression test combines SQLite-preferred
tables, loose DBC fallback, and a Wrath M2 plus external skin with no packed MPQs at all.
That proves the reader paths, not that one small patch folder supplies a complete game install.

## Branches

The fork keeps the upstream line alive, so the 1.12.1 improvements keep flowing in — the shape
Trinity Core gives a version line:

- **`main`** mirrors upstream `main`, the 1.12.1 line. It is a clean tracking branch: fetch the
  upstream, advance `main`, and nothing fork-specific lives on it.
- **`3.3.5`** is the consolidated integration line described above. Build and continue from here.

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

1. **The 3.3.5 wire protocol** — the critical path. `benilla-protocol` and `benilla-srp` still
   speak 1.12.1 build 5875; support for M2 format version 264 does not add Wrath networking.
   The reverse-engineering notes upstream builds against
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
5. **Keeping the fork honest** — merge `main` into `3.3.5` as upstream moves, with real-file
   verification. The features are already integrated here; do not split the existing
   integration back into separate branches.

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
`WOW_USER`/`WOW_PASS` environment variables; with no complete, nonempty pair, use the login screen.
`WOW_HOST` defaults to `127.0.0.1:3724`. For a local host, the launcher first runs
`../vmangos/bin-db/start-server.sh` and waits up to 60 seconds for the auth (3724, or the
port in `WOW_HOST`) and world (8085) listeners. The existing server starter manages MariaDB,
realmd and mangosd and leaves them running after the client exits.
`BENILLA_SERVER_START=/path/to/start-server.sh` selects another configured local stack;
`BENILLA_WORLD_PORT` overrides its world port. The server must already be installed and
configured; the client build script does not provision it. Use `./run-benilla.sh --no-server`
for client-only startup; a remote `WOW_HOST` also skips local server startup.
If `WOW_DATA` is unset, a sibling
`VanillaClient/Data` directory is used when present; otherwise the normal install resolver
applies. An explicitly empty `WOW_DATA` still means no install. The launcher runs Cargo so
changed code is rebuilt, including when `CARGO_TARGET_DIR` points outside the checkout.

If NVIDIA presentation exits with `Couldn't get swap chain texture` on Wayland, or stalls
for about a second per frame on XWayland, disable vertical sync for that run:

```sh
WOW_NOVSYNC=1 ./run-benilla.sh
```

This is an explicit override; the default remains the reference's vertical sync setting.
Alternatively, select XWayland (requires an available X11 `DISPLAY`):

```sh
env -u WAYLAND_DISPLAY ./run-benilla.sh
```

This changes the window backend, not the Vulkan renderer. The launcher does not install or
replace graphics drivers.

The 1.12.1 client runs as upstream — a 1.12.1 install (build 5875), a 1.12.1 server with Warden
off ([vmangos](https://github.com/vmangos/core) is what development runs against), stable Rust
and a C compiler:

```sh
WOW_DATA=/path/to/WoW/Data cargo run --release -p benilla
```

The SQLite DBC source is opt-in by presence: provide a `DBFilesClient` SQLite file in the install's
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

## Continuing on another machine

### Checkout, tools, data and server

Use this fork's existing branch, not upstream `main` or an old feature branch:

```sh
git clone --branch 3.3.5 https://github.com/KonTy/benilla-3.3.5.git benilla
cd benilla
./build-benilla.sh --jobs 2
export PATH="${CARGO_HOME:-$HOME/.cargo}/bin:$PATH"
export WOW_DATA="/path/to/your/client/Data"
```

The pinned toolchain is Rust **1.98.1**, including rustfmt and Clippy. Prefer it over the OS
toolchain; the system Rust 1.99.1 produced different lint/deprecation results. Ubuntu CI
installs real native dependencies and builds all binaries. The Nobara/Fedora and Arch package
manager branches have contract coverage; a native Nobara build has not been verified.
Release builds were verified on an Arch-derived Linux host. macOS was not built in this round.
Do not run the whole bootstrap as root; only its native package installation needs elevation.

Git does **not** bring game data, the optional SQLite database, custom models/textures,
the VMaNGOS checkout, SQL databases, accounts, GPU drivers, or local settings to the next
machine. The optional sibling layout is:

```text
workspace/
  benilla/                         # this checkout and its tracked run-benilla.sh
  VanillaClient/Data/              # your separately supplied data
  vmangos/bin-db/start-server.sh   # your separately configured server starter
```

The parent-directory launcher on the original workstation only delegates to the tracked
launcher. Use this checkout's script on the next machine. The old VMaNGOS starter is external
to this repository and contains machine-specific paths; do not assume it is portable.
Configure a starter for the new machine and select it with `BENILLA_SERVER_START`, or start
the server separately and pass `--no-server`. A local server uses auth port 3724 and world
port 8085 by default; the tested external stack also used MariaDB on 3307. The client launcher
waits for auth/world readiness, not for a particular database implementation.

```sh
BENILLA_SERVER_START=/path/to/configured/start-server.sh ./run-benilla.sh
# Or, when the server is already managed separately:
./run-benilla.sh --no-server
# A remote WOW_HOST automatically skips local server startup:
WOW_HOST=server.example:3724 ./run-benilla.sh
```

Create a dedicated non-GM account in your own server; the launcher does not create accounts.
Use the login screen or supply a complete nonempty `WOW_USER`/`WOW_PASS` pair privately.
Never commit credentials or `.probe-identity`. Empty credentials now leave the login screen
alone instead of accidentally activating automatic login.
For automated runs, follow [the unattended-run rules](docs/CONTRIBUTING.md#setting-up):
declare the checkout's test identity, set `WOW_UNATTENDED=1` and `WOW_NOSOUND=1`, and never
use a player's account or perform unattended combat.

### Data and weapon-model pitfalls

- Loose files need their archive-relative paths beneath a recognized `.MPQ/` or `.patch/`
  directory. A folder ending in `.mpq.extracted` is not automatically that archive mount.
- A real MPQ wins over a folder with the same virtual mount name. Check patch priority and
  the winning source before blaming a parser; use a separate test data tree rather than
  modifying the reference install.
- The weapon fixture is `Item/ObjectComponents/Weapon/Axe_2H_IcecrownRaid_D_01.M2`,
  with sibling `Axe_2H_IcecrownRaid_D_0100.skin`. Supply its referenced BLP textures too.
  It was described as a sword, but do not infer its server item identity from that description.
  No model or texture is bundled in this repository.
- The SKIN has four sections and five draw batches, selecting sections `[0, 1, 1, 2, 3]`.
  Vertex counts per batch are `[24, 957, 957, 32, 4]`. The pipeline test now checks all five
  passes, rather than asserting five submeshes and then expecting only four vertex counts.
- Loading the asset is not an equip test. No server item/template/display ID or in-game weapon
  equip procedure has been verified. Establish the server item and matching ItemDisplayInfo
  reference before trying an equip command; do not invent an item ID.

### Verified baseline and graphics findings

Implementation baseline: **`9e2aad8`**, with server-first launching in `148c347` and encrypted
MPQ support in `f0254d3`. This is a validation checkpoint, not a claim that every gate is green.

| Check | Result |
| --- | --- |
| All workspace release binaries on the Linux workstation | Passed |
| Formats, M2 and MPQ library tests with real install data | 516 + 32 + 20 = **568 passed** |
| Real-data Wrath model pipeline test | Passed after the five-batch correction |
| Build/launcher contracts | **21 passed**, including server ordering and real socket readiness |
| Workspace rustfmt and all-target Clippy with warnings denied | Passed |
| Full tests against the modified install | Failed; eight other reference assertions remain below |
| Ubuntu implementation CI | [Run for `9e2aad8`](https://github.com/KonTy/benilla-3.3.5/actions/runs/39733786732); check its final conclusion |
| Native Nobara, macOS, full gameplay/combat and weapon equipping | Not verified |

Ubuntu CI has no game install or display: its green result cannot prove rendering, live
networking, or the data-gated cases that skip there. It covers bootstrap/build, contracts,
formatting, Clippy, no-install workspace tests, static/reference checks and player-feature tests.

The actual client initially rendered the login scene but raised
`CharacterCreateRandomName` being nil. `patch-b.mpq` supplied an encrypted
`Interface/GlueXML/CharacterCreate.xml` that the old reader rejected. After the decryption fix,
that entry decoded to 41417 bytes of valid XML with exactly one required random-name frame;
the login and character-selection screens rendered without that error dialog.
Do not mistake an engine-only `benilla-worldview` boot check for a client login/render test.

There is a separate presentation problem on the tested NVIDIA RTX 3070 Ti, driver 615.78.08,
Vulkan/Hyprland setup: default vsync repeatedly caused a native Wayland swapchain-acquisition
timeout. XWayland rendered, but showed approximately one-second frame stalls with vsync.
`WOW_NOVSYNC=1` kept native Wayland rendering without those repeated stalls: a 25-second
authenticated character-selection run and an adjacent 20-second manual-login run completed
cleanly with screenshots. This is a verified workaround, **not a fix to the default driver/
presentation path**; do not silently change the reference's vsync default.

A dedicated non-GM account passed SRP, realm connection and world-server authentication to
an empty character list. The later client log also records a roster with one character, world
entry and a server-acknowledged logout, followed by exit code 0. That is connection evidence,
not a validated world-rendering, gameplay, combat or equipment test. The client is no longer
open; the server listeners were still responding after it exited. Processes and account state
are local to that workstation, not restored by cloning this repository.

### Remaining reference failures and next steps

The broad real-install run initially failed nine assertions in six targets; the Wrath batch
assertion above was corrected and its exact test passed. The eight remaining observations
below must be reproduced and classified, not hidden by weakening expected reference values:

| Target / test | Observed versus expected |
| --- | --- |
| [`benilla-app --lib`: `the_stock_glue_loads_with_only_unbuilt_verbs_raising`](crates/benilla-app/src/ui_script/glue.rs) | `Frame TOSText: Unknown element type: h4` |
| [`benilla-app --lib`: `the_stock_create_makes_the_character_and_selects_it`](crates/benilla-app/src/ui_script/glue_mgr_tests.rs) | Reset draws `(7, 0, 1)` versus `(1, 0, 1)` |
| [`benilla-app --lib`: `hovering_the_indicator_shows_and_live_updates_the_game_time_tooltip`](crates/benilla-app/src/ui_script/minimap_tests.rs) | `9:07 PM` versus `21:07` |
| [`benilla-app --lib`: `the_production_load_stays_inside_the_1_12_surface`](crates/benilla-app/src/ui_script/surface_gate.rs) | Extra global `LOCALE_enUS` |
| [`benilla-app --test reference_strings`](crates/benilla-app/tests/reference_strings.rs) | `Interface/FrameXML/Localization.lua` contributed no sentences |
| [`benilla-formats --test chain`: `decodes_m2_collision_hulls`](crates/benilla-formats/tests/chain.rs) | ElwynnTreeCanopy01 collision hull: 412 triangles versus 52 |
| [`benilla-formats --test m2_billboard_card_face`](crates/benilla-formats/tests/m2_billboard_card_face.rs) | CheeseShop01: zero billboard cards versus two |
| [`benilla-formats --test m2_sequence_bounds`](crates/benilla-formats/tests/m2_sequence_bounds.rs) | Wolf Stand bounds radius 1.986 versus 1.963 |

The three differing model files were all confirmed to resolve from **`patch-3.patch`**.
Compare a known stock 1.12 chain with the modified chain before changing these assertions or
the renderer. Investigate the UI/localization differences independently; their presence in a
modified install alone does not prove a harmless fixture difference or an engine defect.
Do not delete patches or edit install files to make a test green.

Reproduce the already-passing reader and weapon-pipeline checks with your own data:

```sh
CARGO_TARGET_DIR=target/feature-check CARGO_BUILD_JOBS=2 \
  BENILLA_REQUIRE_DATA=1 WOW_DATA="$WOW_DATA" \
  cargo test --locked -p benilla-formats -p benilla-m2 -p benilla-mpq --lib
CARGO_BUILD_JOBS=2 BENILLA_REQUIRE_DATA=1 WOW_DATA="$WOW_DATA" \
  cargo test --locked --workspace --lib tests::loads_a_wrath_m2_through_the_pipeline -- --exact
python3 scripts/test-tools.py
```

The weapon test requires that fixture M2 and its external skin in your own data chain.
Keep the full workspace feature graph for its command to avoid expensive Bevy rebuilds from
switching feature sets; the separate target directory keeps the smaller reader loop isolated.
Then run `WOW_DATA= cargo test --locked --workspace` for the no-install cases, and
`CARGO_BUILD_JOBS=2 scripts/gates.sh` with the install, addon corpus, reference checkout and
display configured as [CONTRIBUTING](docs/CONTRIBUTING.md#setting-up) describes. Record skips
and exact failures. A complete `scripts/smoke.sh` run, including its install-write audit, has
not been established by the GUI login probes.

On the next machine, prioritize native Ubuntu/Nobara bootstrap validation, reproducing and
classifying the eight failures, and the NVIDIA default-vsync issue before claiming all gates
green. Old diagnostic logs/screenshots are untracked and do not travel with Git; reproduce the
evidence locally. Keep outputs outside the game install and credentials private.
The longer-term Wrath wire/map/UI work remains [separate work](#jump-in-here): this branch
still connects using **1.12.1**, not the 3.3.5 network protocol.

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
