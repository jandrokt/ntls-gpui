# ntls

A desktop toolbox for the network questions you ask most often — is this host
up, what else is on this network, what is it listening on, what does that
endpoint answer — and a place to keep the answers.

![The ntls window: a workspace in the side bar, an IP scan and its results](docs/screenshot.png)

Ten tools, one window. Everything a tool finds is a file on disk, several tools
make a workspace, and a workspace can hold notes and workflows that read what
the tools found. It runs on **macOS, Linux and Windows**, on Intel and ARM.

Built with [GPUI](https://gpui.rs), the GPU-accelerated UI framework behind Zed.

---

## Contents

- [The tools](#the-tools)
- [Getting it](#getting-it)
- [Basic usage](#basic-usage)
- [Workspaces are directories](#workspaces-are-directories)
- [Documents](#documents)
- [Workflows](#workflows)
- [Scripting: the expression language](#scripting-the-expression-language)
- [Variables](#variables)
- [Keyboard](#keyboard)
- [What needs privileges, and where](#what-needs-privileges-and-where)
- [Building](#building)
- [Adding a tool](#adding-a-tool)

---

## The tools

| Tool | What it does |
| --- | --- |
| **Ping** | Probes one host continuously, with live loss, min/avg/max/mdev and a latency graph. |
| **Traceroute** | Maps the routers between here and a host, probing hops in parallel so a full trace takes seconds. |
| **IP scan** | Sweeps a subnet or range and lists the hosts that answer, with reverse DNS, MAC addresses and vendors. |
| **Port scan** | Checks TCP, UDP or both, by list, range or all 65535, and reads service banners. |
| **DNS lookup** | Resolves names and addresses against any server: A, AAAA, CNAME, MX, NS, TXT, SOA, SRV, PTR. |
| **HTTP request** | Sends a request and reports the status, timing, size and headers — once, or on an interval with a graph. |
| **Subdomains** | Lists a domain's subdomains from certificate transparency logs, with search and sorting. |
| **Internet speed** | Download, upload and latency against a public endpoint, or any URL you name. |
| **LAN throughput** | Real speed between two machines running ntls, with the peer found by broadcast. |
| **Download** | Paste one link or twenty and it works out what each is and fetches them, several at a time, resumable. |

Ping and IP scan work over **ICMP** or **ARP**. ICMP reaches anything routable;
ARP only works on your own link but finds hosts that ignore pings, and is the
only way to learn what a device actually is. ntls speaks both itself rather
than shelling out, so a whole subnet sweep shares one socket.

---

## Getting it

Every push builds all six targets, and a `v*` tag turns the same artifacts into
a release:

| System | What you get |
| --- | --- |
| **macOS** | `ntls.app` in a zip — one universal build for Intel and Apple silicon |
| **Windows** | An installer, and a portable zip that is the `.exe` and nothing else — x64 and ARM64 |
| **Linux** | A tarball with the binary, an icon and a `.desktop` file, plus an `install.sh` that puts them under `~/.local` |

The macOS build is ad-hoc signed rather than notarised, so the first launch
needs **right-click → Open**, or:

```
xattr -dr com.apple.quarantine ntls.app
```

Or build it yourself — see [Building](#building).

---

## Basic usage

**A workspace is one investigation.** It holds the tools you opened, what they
found, and any notes and workflows you wrote about them. The tab strip switches
between investigations rather than between tools.

**Add a tool.** `⌘K` (`Ctrl+K` off macOS) opens the command bar. Type a tool's
name and press return for its form, or type the whole thing and skip the form:

```
ping 10.0.0.1
ping 10.0.0.1 count=10 interval=500ms
ipscan 10.0.0.0/24 method=arp
portscan 10.0.0.31 ports=top
dns example.com type=MX
http https://example.com/health count=5
```

The first word picks the tool. Bare words fill the field the tool marks as its
target; `key=value` fills any other field, by its key or by the label on
screen. The bar shows what return will do before you press it, and `⇥` fills in
whatever is highlighted.

`⇥` also expands shorthand in place, one step at a time, so you can see exactly
what is about to happen:

```
auto  →  10.0.0.0/24  →  10.0.0.1-10.0.0.254
all   →  1-65535
top   →  21-23,25,53,67-69,80,110,111,123,135,137-139,…
```

**The same box searches everything open** — a tool by name, a workspace, a
document by a line inside it, a workflow by a run it starts, an interface, and
every row of every set of results. An address found last week is one query
away, and choosing it opens the workspace, the tool and the row it is in.

**Running.** `⌘R` runs, `⌘.` stops. A scan that is stopped keeps what it found
and offers **Resume** (carry on, adding to the table) and **Restart** (throw it
away and do the whole thing again). The IP scan and the DNS lookup also have a
**Keep earlier results** switch: a run then adds to the table instead of
replacing it, so a host that has gone quiet keeps its row and an address a
different device has taken over gets a row of its own beside it.

**Reading results.** Click a heading to sort, drag its edge to resize, `⌘F` to
filter. Every row has a **NOTE** you can type into, and notes belong to the
workspace rather than to the run — a remark made while reading a sweep is still
there in the port scan that follows it. Selecting a row reveals a **Send** bar:
pick a host out of a sweep and one click lands it in the port scanner, port and
all.

**Comparing.** *Compare with…* in a run's menu reads it against another run of
the same tool: what appeared, what went, and what changed, with the previous
value struck through beside the current one.

**Exporting.** *Export as CSV…* writes what is on screen — the tool's columns,
the rows in the order they are sorted and filtered into, and the notes.

**Right-click anything.** A workspace, a tool, a document, a workflow, a group
heading, a result row, an interface, the background. The menu is about what is
under the pointer: run it, rename it, colour it, star it, reveal it, copy it,
send it somewhere, delete it.

---

## Workspaces are directories

A workspace is a directory, and everything in it is a file:

```
~/Documents/ntls/
  office-lan/
    workspace.json      the name, the colour, the notes, the variables
    001-ipscan.json     one tool: its settings, and its results
    002-portscan.json
    report.md           a document
    nightly.flow        a workflow
    perimeter/          a folder, holding more of the same
      003-dns.json
```

Everything is written as it changes and read back at startup, so results from
last week are still there — rows, log, charts and all. You can open the
directory in your file manager, copy one somewhere, or delete it by hand.

The side bar lists **folders first**, then what is loose in the workspace,
grouped by the stage each tool is at — *Not run*, *Running*, *Results* — with
the documents and workflows under them. Every heading can be emptied, and
emptying one empties the heading you clicked rather than every heading of that
name.

The **page button** at the top of that side bar switches it to a **file view**:
the same workspace, listed the way the filesystem holds it — every file, its
real name and size, the folders they sit in, and `.closed` where anything
removed went. Clicking a file shows what it is; a file the workspace does not
hold is handed to your file manager.

Nothing is destroyed by a click: removing a tool, a document or a workflow
moves its file into the workspace's own `.closed` folder. Deleting a whole
workspace is the one action that destroys anything, and it asks first.

---

## Documents

**New document** writes an empty `.md` file beside the tools and opens it here,
split: the source on one side, what it comes to on the other, updating as you
type.

What makes a document more than a text file is that anything between double
braces is an expression, worked out against the tools in the same workspace
every time the document is shown.

![A document, its expressions worked out against the runs beside it](docs/document.png)

The document above is written like this:

```markdown
# Office LAN, {{ subnet }}

The sweep found **{{ Sweep.up }}** hosts on {{ subnet }} in
{{ fixed(Sweep.elapsed, 1) }} s. The gateway at {{ gateway }} answered
{{ Uplink.recv }} of {{ Uplink.sent }} probes ({{ Uplink.loss }} loss),
averaging **{{ fixed(Uplink.rtt.avg(), 2) }} ms**.

- Slowest host on the sweep: {{ fixed(Sweep.rtt.max(), 1) }} ms
- Open ports on the NAS: {{ "Ports on the NAS".rows }}
- Health endpoint: {{ fixed("Status page".time.avg(), 1) }} ms average

> Verdict: {{ if Uplink.rtt.avg() < 5 then "healthy" else "the gateway is slow" }}.
```

A tool that has not run yet is *nothing* rather than an error, so a document
written before the scan still renders. An expression that is actually wrong is
left where it was written, in red, saying why.

The editor colours what you are writing and **completes what it knows**: the
runs in the workspace, the variables, the columns and summary figures of
whichever run you named before the dot, the fields every run has, and the
functions of the expression language. `⇥` takes the highlighted suggestion,
`↑↓` walks them, `esc` dismisses.

*Open in another editor…* hands the file to whatever else you write markdown
with. Nothing else does that unasked.

---

## Workflows

A question about a network is rarely one tool, and rarely the same tools every
time — you sweep, and *then* scan the ports of whatever answered, or give up if
nothing did. A workflow writes that down, and you build it with the mouse.

![The workflow editor: steps, conditions and a variable being set](docs/workflow.png)

- **Add step** offers the six kinds there are, and every block ends in a faint
  `+ step` that puts one inside a branch or a repeat.
- **Click a step** and its controls appear on its own line: which run a `run`
  starts, how many passes a `repeat` makes, how long a `wait` waits, what a
  `set` works out and what it keeps it under.
- **The condition is chosen, not typed.** *only if* opens four controls — which
  run, which of its figures, how to compare, and what to compare with — and
  each offers what actually exists: the runs in this workspace, then that run's
  own columns and the figures it reported.
- `⌃`/`⌄` move a step among its neighbours, `+` opens the rest of what can be
  done to it, `×` removes it. Clicking past the steps, or `⏎`, or `esc`, puts
  the controls away.
- Each kind of step has a colour — blue does the work, amber decides, green
  goes round again, grey waits, red ends it.

Underneath, a workflow is a text file, and the two are the same thing: **Text**
shows the source and edits it directly, changes made there appear in the
controls as you type, and changes made with the controls appear in the text.

```text
# Nightly check

run "Sweep"
set hosts_up = Sweep.up
if Sweep.up > 0 {
  run "Ports on the NAS"
  wait 30s
  run "Status page"
} else {
  run "Uplink"
}
stop if Sweep.down > 20
```

| Step | What it does |
| --- | --- |
| `run "Name"` | Starts a run in this workspace by name and waits for it to finish. |
| `if … { } else { }` | Takes one branch or the other. |
| `repeat 3 { }` | Does the same thing a fixed number of times. |
| `wait 30s` | Pauses. |
| `set name = expression` | Works something out and keeps it under a name. |
| `stop` | Ends the workflow. |

`run` and `stop` can carry an `if` of their own. Every condition is worked out
**when the step is reached**, not when the workflow starts — which is the whole
point: a step sees what the steps before it found. A step whose condition is
false is skipped and says so; a run that fails stops the workflow, because
carrying on would be acting on results that are not there.

---

## Scripting: the expression language

The same language runs inside `{{ }}` in a document, in a workflow's
conditions, and on the right of a workflow's `set`.

**Naming a run.** By the name you gave it, or by its tool:

```
Sweep.up          the run called Sweep
ipscan.rows       the one IP scan in this workspace
"Port scan".open  a name with a space in it goes in quotes
tool("Port scan") the same thing, written the long way
```

**What a run answers to:**

| | |
| --- | --- |
| `rows` `up` `down` `warn` | how many results, and how they went |
| `target` `state` `ok` `elapsed` `name` | what it was pointed at, where it got to, how long it took |
| any column, by name | `Sweep.rtt`, `Sweep.host`, `Sweep.vendor` — a list, one entry per row |
| any figure from its summary | `Uplink.loss`, `Sweep.scanned` — whatever the tool reported |

Columns are text with units, and the arithmetic reads the figure out of them,
so `Sweep.rtt.avg()` averages `"12.4 ms"` and skips the rows that got no reply.

**Functions.** A method is the same thing written the other way round, so
`avg(Sweep.rtt)` and `Sweep.rtt.avg()` are one expression:

```
avg  min  max  sum  count  median  p95  percentile
round  floor  ceil  abs  fixed  percent
first  last  join  text  upper  lower  number
exists  tool  tools
```

There is `if … then … else …`, the usual arithmetic and comparisons, and
`&&` `||` `!` (or `not`). There are no loops, no definitions and no side
effects: an expression reads what was found and says what it means.

```
{{ if Sweep.up == 0 then "nothing answered" else Sweep.up + " hosts" }}
{{ fixed(percent(Sweep.up / Sweep.rows), 1) }}
{{ Sweep.host.join(", ") }}
{{ exists(tool("Nightly sweep")) }}
```

---

## Variables

A variable is a named value belonging to the workspace rather than to any run
in it. Every document, condition and workflow in the workspace reads it by
name, and there are two kinds:

| Kind | What it holds | When it changes |
| --- | --- | --- |
| **Text** | what you typed, or what a workflow kept | when something writes it |
| **Formula** | an expression | never: it is worked out afresh every time it is read |

The difference is the difference between a fact somebody wrote down and one
that stays current. `subnet = 10.0.0.0/24` is text. `gateway =
Sweep.host.first()` as a formula follows the sweep; as text it is whatever the
sweep said the day it was written.

### The editor

The **Variables** section of the side bar is where they live. Each row shows
the name, **what it comes to now** — a formula shows its answer, with the
expression underneath — and a line saying where the value came from and who
reads it:

```
gateway     10.0.0.1                       =  ×
            = Sweep.host.first()
            read by Office LAN, 10.0.0.0/24

hosts_up    10                             =  ×
            set by Nightly check · 8 min ago

subnet      10.0.0.0/24                    =  ×
            the network this workspace is about
```

Click the name to rename it, the value to retype it, the bottom line to say
what it is for, and `=` to switch between text and formula — the same text is
kept either way, so an expression you typed as text starts working the moment
you press it. A formula that cannot be worked out says why, in red, where its
answer would be. One that names itself, directly or through another, answers
with nothing rather than going round for ever.

*Read by* is counted from the documents, workflows and other formulas that
actually name it, so a variable nothing reads says so.

### Workflows set them

A `set` step works an expression out **when the step is reached**, against
everything found so far, and keeps the answer:

```text
run "Sweep"
set hosts_up = Sweep.up
run "Ports on the NAS" if hosts_up > 0
```

While you build it, the step shows what it would keep — `set hosts_up =
Sweep.up → 10` — and the name is chosen from the variables the workspace
already has. When it runs, the trail against that step says what it set, and
the variable remembers which workflow wrote it and when.

A condition can be about a variable as easily as about a run: choosing a
variable as the subject drops the "which figure" control, because a variable is
a value already — `if [hosts_up] [is more than] [0]`.

A run and a variable can share a name; the run wins, so a document that names
one means the run.

---

## Keyboard

Shortcuts are written here the way macOS writes them; on Windows and Linux
every ⌘ is **Ctrl**.

| Where | Keys |
| --- | --- |
| Adding | `⌘K` the command bar · `⌘1`–`⌘9` add that tool straight away |
| Workspaces | `⌘T` new · `⌘⇧W` close · `⌘⇧[` / `⌘⇧]` previous/next |
| Tools | `⌘[` / `⌘]` previous/next · `⌘W` close the tab (the tool stays) |
| A run | `⌘R` run · `⌘.` stop · `⌘E` settings · `⌘I` rename · `⌘G` enlarge the graph |
| Layout | `⌘B` side bar · `⌘J` output panel · `⇧⌘O` workspaces · `⇧⌘E` tools |
| Results | `↑↓` select · `pgup`/`pgdn` page · `⌘↑`/`⌘↓` first/last · `⌘F` filter · `⌘⇧N` note · `⏎` send the selection onward |
| Anywhere | `⌘D` light/dark · `esc` back out · `⌘Q` quit |

---

## What needs privileges, and where

Everything works everywhere except where the operating system reserves it:

| | macOS | Linux | Windows |
| --- | --- | --- | --- |
| **ICMP** ping, traceroute, ICMP sweep | unprivileged | unprivileged where `net.ipv4.ping_group_range` allows it, otherwise root | needs **Administrator**: Windows has no unprivileged ICMP socket |
| **TTL** column | yes | yes | blank — a Windows datagram socket does not carry it |
| **ARP** sweep, hardware addresses | real ARP over BPF with ChmodBPF, otherwise the neighbour table | the neighbour table from `/proc/net/arp` | the neighbour table from `arp -a` |
| Port scan, DNS, HTTP, subdomains, speed tests, downloads | yes | yes | yes |

Hardware addresses are resolved against the IEEE registries — about 54,000
prefixes, compiled into the binary — so lookups are instant, work offline, and
never tell anyone what you are scanning. Addresses a device made up for itself
are reported as `randomised` rather than guessed at.

The **Download** tool uses [`yt-dlp`](https://github.com/yt-dlp/yt-dlp) and
[`gallery-dl`](https://github.com/mikf/gallery-dl) if they are on your `PATH`,
and says so in the log if they are not. Neither is required or bundled.

---

## Building

```
cargo build --release
cargo test
```

Rust 1.85 or newer. On macOS, the Metal toolchain GPUI compiles its shaders
with:

```
xcodebuild -downloadComponent MetalToolchain
```

On Debian and Ubuntu, what GPUI links against:

```
sudo apt install libasound2-dev libfontconfig-dev libwayland-dev \
    libx11-xcb-dev libxcb1-dev libxcb-dri3-dev libxkbcommon-x11-dev \
    libssl-dev libzstd-dev libvulkan-dev make cmake clang
```

On Windows, the MSVC toolchain — nothing else.

The tests are hermetic: they exercise target and port parsing, the certificate
transparency aggregation, the embedded vendor database, the tool contract, the
expression language, the workflow reader, writer and machine, and a real port
scan and ping against the loopback interface. Nothing in them touches the
network beyond this machine.

**Packaging.** `packaging/icon/make-icons.py` draws the application icon and
writes every form the three systems want from one drawing;
`packaging/macos/bundle.sh` puts a built binary in a `.app`;
`packaging/windows/ntls.iss` is the Inno Setup script;
`packaging/linux/` holds the `.desktop` file and the tarball's installer.
`.github/workflows/build.yml` runs all of it.

---

## Adding a tool

The interface knows nothing about pinging or port scanning. It renders a form
from whatever fields a tool declares and a table from whatever columns it
declares, so a new tool is one file and one line.

```rust
pub trait Tool: Send + Sync + 'static {
    fn id(&self) -> &'static str;
    fn title(&self) -> &'static str;
    fn desc(&self) -> &'static str;
    fn icon(&self) -> &'static str;
    fn fields(&self) -> Vec<Field>;
    fn columns(&self) -> Vec<Column>;
    fn run<'a>(&'a self, run: Run, emit: Emitter) -> BoxFuture<'a, anyhow::Result<()>>;
}
```

A run emits events — a row, a log line, a summary figure, a progress update, a
numeric sample — and the interface turns those into a table, an output panel, a
stat bar, a progress bar and a live graph without being told to. Implement the
trait in `src/tools/`, add one line to `register`, and the picker, the command
bar, the form, validation, hand-offs, saving, resuming, the CSV export and the
expression language all follow.

---

## Licence

Apache-2.0. See [LICENSE](LICENSE).
