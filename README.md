# ntls

A desktop app for the network questions that come up over and over: is this
host up, what else is on this subnet, what is it listening on, what does that
endpoint actually return. Whatever it finds gets saved as a file in a directory
you can open, and you can write notes and small automations on top of the
results.

![The ntls window, showing a subnet sweep and its results](docs/screenshot.png)

Runs on macOS, Linux and Windows, on Intel and ARM. Written in Rust using
[GPUI](https://gpui.rs).

## Tools

| Tool | What it does |
| --- | --- |
| Ping | Probes one host continuously. Live loss, min/avg/max/mdev, latency graph. |
| Traceroute | Maps the routers to a host. Hops are probed in parallel, so a full trace takes seconds. |
| IP scan | Sweeps a subnet or range, lists what answers, with reverse DNS, MAC and vendor. |
| Port scan | TCP, UDP or both. A list, a range, or all 65535, with banner reading. |
| DNS lookup | A, AAAA, CNAME, MX, NS, TXT, SOA, SRV, PTR, against any server you like. |
| HTTP request | Sends a request, checks the answer is what you expected, pulls a value out of it. |
| Subdomains | Subdomains from certificate transparency logs, searchable and sortable. |
| Internet speed | Download, upload and latency against a public endpoint or a URL you name. |
| LAN throughput | Actual speed between two machines running ntls. The peer is found by broadcast. |
| Download | Paste one link or twenty. It works out what each one is and fetches them, resumably. |

Ping and the IP scan can work over ICMP or ARP. ICMP reaches anything routable.
ARP only works on your own link, but it finds devices that ignore pings, and
it's the only way to get a real hardware address out of one. Both are spoken
directly instead of shelling out to `ping` or `arp`, so a /24 sweep runs on a
single socket.

### HTTP checks

The HTTP tool sends whatever you tell it to: any method, query parameters
(escaped for you), headers, Basic or Bearer credentials, a body as text, JSON,
XML or form. It can go through a proxy, follow redirects or not, force
HTTP/1.1, and skip certificate checking for the appliance in the rack with a
self-signed cert.

The body gets an editor rather than a one-line box, coloured for whichever
body type you picked, so a payload of any size is something you can read.
**Attachments** takes one file per line, `/tmp/report.pdf` or
`name=/tmp/report.pdf` to choose what the part is called; sending any turns
the request into `multipart/form-data`, and a `form` body is split into
fields alongside the files.

Paste a `curl` line into the URL box and the rest of the form fills itself:
the method, the headers, the body and its type, the credentials, and the
switches that change the request. A bearer token arrives as a header and
becomes the credential, where you would think to look for it. Anything the
line does not say is left as it was, so a `curl` without `-k` doesn't turn
off a switch you had on. That is how an HTTP request is usually handed
round — in an issue, in a service's own docs, off a browser's *copy as
cURL* — and retyping one a field at a time is the tedium the form was
supposed to remove.

**Response** shows what came back: the status, the content type, the size,
and the body itself. JSON is pretty-printed and coloured, XML is coloured,
and anything else is left alone but set in a monospaced face so columns line
up. **Headers** swaps the body for what came back with it, **Raw** leaves the
body exactly as it arrived when the question is about the bytes, and **Copy**
takes it away with you.

Beside the size are the two halves of the time it took: **waited**, which is
everything up to the first byte — name resolution, the connection, the
handshake, and the far end's own thinking — and **read**, which is the answer
arriving. A server thinking for a second and a megabyte crawling down a slow
link are different problems, and one number for both can't tell you which one
you have.

Two fields turn a request into a check. **Expect** says what a good answer
is: `any`, `2xx`, `404`, `200-204`, or a list. Anything else turns
the row red and says what you asked for. **Capture** pulls one value out of
every response into its own column, either a path into the JSON (`data.queue`,
`items[0].id`), a header (`header:X-Request-Id`), the status, or the raw body.

If what you capture is a number it gets graphed next to the response time, and
it shows up in the run's summary, so `{{ "Status page".value.last() }}` works
in a document. Set the request count to 60 with a one minute interval and
you've got a monitor.

Capture is for one value per request, on every request. For the rest of the
answer, a document or a workflow can reach into the last one directly:

```
{{ http.response.status }}                  200
{{ http.response.type }}                    application/json
{{ http.response.headers["x-request-id"] }} a header, by a name a dot can't spell
{{ http.json.queue.depth }}                 straight down into the JSON
{{ http.json.hosts[0] }}                    an array in it is a list
{{ http.json.keys }}                        what came back, without guessing
{{ if http.response.status == 200 then "up" else "down" }}
```

Name the run instead of the tool (`"Status page".json.queue.depth`) when the
workspace has more than one. A property that isn't there is nothing rather
than an error, so a service that omits a value on a quiet day doesn't stop
the document from rendering.

You don't have to know the shape in advance. Type a dot after the run and
completion lists what it answered beside its columns, with what's actually
there next to each name; keep going and it walks down into the answer a level
at a time. The workflow's condition controls offer the same paths under *What
it answered*.

## Installing

Every commit becomes a release, tagged `build-<n>`:

- **macOS** `ntls.app` in a zip, one universal build for Intel and Apple
  silicon.
- **Windows** an installer, plus a portable zip that's just the `.exe`. x64 and
  ARM64.
- **Linux** a tarball with the binary, an icon, a `.desktop` file and an
  `install.sh` that drops them under `~/.local`. x86-64 and aarch64.

## Builds, not versions

ntls doesn't have a version number. It has a build number, and every commit
gets the next one. It's `github.run_number`, the workflow's own count, so it
goes up by one each time and never repeats. It gets stamped into the binary at compile time, it's
what the program calls itself on the welcome screen and in Settings, it names
every artifact, and it's the tag on the release those artifacts hang off. Build
412 is one commit, one pipeline, one set of binaries.

A build you make yourself has no number and says `local build`. A binary
claiming to be build 412 should be the one CI made. To stamp one in by hand:

```
NTLS_BUILD=412 cargo build --release
```

The `version` in `Cargo.toml` is `0.0.0` and stays there. Cargo insists the
field exists; nothing reads it.

The macOS build is ad-hoc signed, not notarised, so the first launch needs a
right-click, then Open. Or:

```
xattr -dr com.apple.quarantine ntls.app
```

Or build it yourself, see [Building](#building).

## Using it

A workspace is one investigation. It holds the tools you opened, what they
found, and any notes or workflows you wrote about them. The tabs across the top
switch investigations, not tools.

Press `⌘K` (`Ctrl+K` off macOS) for the command bar. Type a tool's name and hit
return to get its form, or type the whole thing and skip the form entirely:

```
ping 10.0.0.1
ping 10.0.0.1 count=10 interval=500ms
ipscan 10.0.0.0/24 method=arp
portscan 10.0.0.31 ports=top
dns example.com type=MX
http https://example.com/health count=5 expect=2xx
```

Bare words go into whatever the tool calls its target. `key=value` fills in
anything else, matched against either the setting's key or its label. The bar
shows you what return is about to do before you press it.

Tab is worth knowing about. It completes whatever is highlighted, and it also
expands shorthand a step at a time so you can see and edit what you're about to
run:

```
auto  →  10.0.0.0/24  →  10.0.0.1-10.0.0.254
all   →  1-65535
top   →  21-23,25,53,67-69,80,110,111,123,135,137-139,…
```

That same box searches everything you have open, not just the tool list:
workspaces, open tools, documents by a line inside them, workflows by a run
they start, network interfaces, and every row of every result table. An address
you scanned last week is one query away, and picking it opens the workspace,
the tool and the row.

It matches five ways, best first: the whole of what you typed, the start of a
name, the start of a word inside one, anywhere inside one, and finally its
letters in order but not together — so `psc` finds the port scan. Whichever
letters answered are picked out in the row, because a match three words in
otherwise looks like no match at all. Answers arrive under a heading each,
with the heading holding the best answer first.

And it does things, not only finds them. Everything in the menus is in there —
run, stop, duplicate, export, compare, close, rename the workspace, switch
between light and dark — named after whatever is in front of you, so it reads
`Run Port scan` and not `Run`. Each shows the keys that also do it, which is
the bar teaching them rather than replacing them. A command that would do
nothing isn't offered: no Stop while nothing runs, no *Next workspace* when
there is only one.

`>` narrows the list to those, when a word like `run` would otherwise name a
tool, a run and a command at once. With nothing typed at all, the bar opens on
what you last used.

Every tool's form puts the question in front and folds the rest away. What
you came to change — the target, and the one or two knobs a run actually
turns — is what you see; timeouts, concurrency, retries and the like sit
under **More settings**. A fold that holds something set away from its
default says how many and opens itself, so it can't hide a setting the run
will act on. The help for whichever field has the caret has one line at the
foot of the form, rather than a reserved line under every field.

A tool's pane is one shape whatever the tool: what it is called and what it
is pointed at on the left, and on the right what state it is in, one control
for switching between the table, the graph, the response and the settings,
and the run buttons. Only the switches that mean something are there — a tool
that never draws a graph is not offered one.

The window keeps its affordances out of the way until you want them. A tab's
close cross and a side bar row's appear when the pointer is on them, and stay
on the one you are looking at; a group heading's *Delete all* appears when the
pointer is on the heading. Four standing offers to delete everything in a
group is a lot of destruction to leave lying around a list you read all day.

`⌘R` runs, `⌘.` stops. Stopping keeps what was found, and the buttons become
Resume and Restart. The IP scan and DNS lookup also have a *Keep earlier
results* switch, which turns a scan into a record of a network over time: a new
run adds to the table instead of replacing it, every row gets a SEEN column
saying which scan last found it, a host that's gone quiet keeps its row and is
marked amber and dated (`1 scan ago`, `2 scans ago`), and an address a
different device has taken over gets a second row next to the first. Rows about
addresses outside the range you just swept are left alone; scanning one host
says nothing about the rest of the subnet.

While a scan is running, a table you're already at the end of stays there as
rows arrive, at the bottom or the top, whichever end you're reading. Scroll
away and it leaves you where you are.

Click a column heading to sort, drag its edge to resize, `⌘F` to filter. Every
row has a NOTE you can type into. Notes belong to the workspace, not to the
run, so something you jotted down while reading a sweep is still there in the
port scan you do next. Select a row and a Send bar appears: pick a
host out of a sweep, one click, and it's in the port scanner with the port
filled in.

There's also *Compare with…* in a run's menu, which diffs it against another
run of the same tool (appeared, went, changed), and *Export as CSV…*, which
writes out what's on screen, filter and sort order included.

Right-click things. Workspaces, tools, documents, workflows, group headings,
result rows, interfaces, the background. The menu is about whatever is under
the pointer.

Runs keep going in workspaces you aren't looking at, so when one finishes it
says so in the corner and the bell in the status bar counts what you haven't
read. A failure (a run, a file that wouldn't write) stays there until you
dismiss it, and clicking a notice opens the run it's about. `⌘⇧M` opens the
list.

## Pages

The rail down the left edge is everywhere the window goes: workspaces, tools,
variables and this machine at the top, the gear at the foot. Click one to go
there. Hover one for its name.

Workspaces and tools are lists, so they fill the side bar. **Variables**, **This
machine** and **Settings** are pages, and take the whole window: none of them
is a thing inside the workspace, so a list of tools beside them would just be
in the way.

**Variables** shows what each one comes to, the formula underneath, what kind
of thing a formula answers with, where the value came from, what reads it and
what it reads. Click any part of a row to edit it in place; hover for copy,
copy-as-`{{ reference }}`, duplicate and remove. A filter box narrows a long
list, and the header counts the ones that aren't working.

**This machine** lists every interface: the network each address sits in, its
broadcast address, how many hosts it holds, the hardware and who made it, which
one carries the default route, and which one new tools send from. Each network
has a Sweep button next to it.

**Settings** (`⌘,`) holds the theme (system, light or dark), whether the
interface animates, whether the side bar comes back the way you left it, which
interface, reachability method, timeout and concurrency new tools start on,
whether they resolve names and identify hardware, whether a table follows new
rows, what the corner says and for how long, and where the workspaces are on
disk. Scanning settings are stored per *field*, not per tool, so every tool
with an interface setting starts on the one you chose, including one added
later.

## Files on disk

A workspace is a directory. Everything in it is a file:

```
~/Documents/ntls/
  office-lan/
    workspace.json      name, colour, notes, variables
    001-ipscan.json     one tool: its settings and its results
    002-portscan.json
    report.md           a document
    nightly.flow        a workflow
    perimeter/          a folder with more of the same
      003-dns.json
```

Writes happen as things change and everything is read back at startup, so last
week's results are still there, rows and log and charts included. Copy a
workspace somewhere, delete one by hand, whatever you like. A write that fails
says so in the corner.

Settings live beside the workspaces in `.ntls.json`, which keeps a workspace
directory to tools and nothing else.

The side bar lists folders first, then whatever is loose in the workspace,
grouped by where each tool has got to (Not run, Running, Results) with the
documents and workflows below. Any heading can be emptied, and it empties the
one you clicked rather than every heading with that name.

The page button at the top of that side bar flips it into a file view: the same
workspace listed the way the filesystem has it, with real names and sizes,
folders, and `.closed`, where anything you remove ends up. A single click
deletes nothing. Clicking a file opens whatever it is, and files ntls doesn't
recognise get handed to your file manager.

## Documents

*New document* makes an empty `.md` file next to the tools and opens it split:
source on one side, rendered on the other, updating as you type.

Anything in double braces is an expression, evaluated against the tools in the
same workspace every time the document is displayed.

![A document with its expressions filled in from the runs beside it](docs/document.png)

That one is written like this:

```markdown
# Office LAN, {{ subnet }}

The sweep found **{{ Sweep.up }}** hosts on {{ subnet }} in
{{ fixed(Sweep.elapsed, 1) }} s. The gateway at {{ gateway }} answered
{{ Uplink.recv }} of {{ Uplink.sent }} probes ({{ Uplink.loss }} loss),
averaging **{{ fixed(Uplink.rtt.avg(), 2) }} ms**.

- Slowest host on the sweep: {{ fixed(Sweep.rtt.max(), 1) }} ms
- Open ports on the NAS: {{ "Ports on the NAS".rows }}
- Health endpoint: {{ fixed("Status page".time.avg(), 1) }} ms average

> Verdict: {{ if Uplink.rtt.avg() < 5 then "healthy" else "gateway is slow" }}.
```

A tool that hasn't run yet evaluates to nothing instead of an error, so you can
write the document before the scan. An expression that's genuinely wrong stays
where it is, in red, with the reason.

The editor highlights as you type and completes what it knows about: runs in
this workspace, variables, the columns and summary figures of whatever run you
named before the dot, whatever it answered, and the language's functions. Tab
accepts, arrows walk the list, escape dismisses.

A toolbar sits above the source with the marks nobody remembers all of:
headings, bold, italic, strikethrough, inline code, bullet and numbered
lists, quotes, links, code blocks, tables, dividers, and `{{ }}`. Each acts on
the selection and each is a toggle, so pressing **B** on bold text takes the
marks off again; a line mark like a heading or a bullet applies to every line
the selection touches. Right-clicking the source offers the same, with the
clipboard above it.

**Preview** hides the rendered half when you would rather have the room to
write, and the edge between the two drags. Dragged far enough it shuts, and
the button puts it back.

*Open in another editor…* hands the file to whatever you normally write
markdown in.

## Workflows

Most network questions take more than one tool, and not always the same ones.
You sweep, and then port scan whatever answered, or give up if nothing did. A
workflow writes that down, and you build it by clicking.

![The workflow editor with steps, a condition and a variable being set](docs/workflow.png)

*Add step* offers the nine kinds. Every block ends in a faint `+ step` for
putting one inside a branch or a loop. Click a step and its controls appear
on its line: which run a `run` starts, how many passes a `repeat` does, how
long a `wait` waits, what a `for each` walks.

Conditions are picked, not typed. *only if* opens four controls (which run,
which of its figures, how to compare, what to compare against) and each one
only offers what actually exists. `⌃`/`⌄` move a step among its neighbours, `+`
opens everything else you can do to it, `×` deletes it. Click empty space, or
press return or escape, to put the controls away.

Steps are colour-coded: blue does work, amber decides, green loops, grey waits,
red stops.

The file underneath is text, and the two stay in sync. *Text* shows the source
and lets you edit it directly, edits there show up in the controls as you type,
and edits with the controls show up in the text.

```text
# Nightly check

run "Sweep"
set hosts_up = Sweep.up
if hosts_up > 0 {
  run "Ports on the NAS"
  wait 30s
  run "Status page"
} else {
  run "Uplink"
}
stop if Sweep.down > 20
```

| Step | Does |
| --- | --- |
| `run "Name"` | Starts a run in this workspace and waits for it |
| `run "Name" with field = …` | The same, having set some of that tool's fields first |
| `if … { } else { }` | Branches |
| `repeat 3 { }` | Repeats a fixed number of times |
| `for each x in … { }` | Once for every item of a list, with the item under `x` |
| `while … { }` | Round again for as long as a condition holds |
| `wait 30s` | Pauses |
| `set name = expression` | Works something out and keeps it |
| `print expression` | Says something into the run log and changes nothing |
| `stop` | Ends the workflow |

`run`, `stop` and `while` can carry their own `if`. Conditions are evaluated
when the step is reached, not when the workflow starts, so a step sees what
the steps before it found. A step whose condition is false is skipped and says
so. A run that fails stops the workflow; carrying on would mean acting on
results that don't exist.

### Doing something to each of them

A workflow that can only run tools exactly as they were left is a list of
buttons. `for each` and `with` are what make it a program: walk what the last
run found, and point the next tool at each one.

```text
# Scan whatever answered

run "Sweep"
log "sweeping found " + Sweep.up + " hosts"

for each host in Sweep.host {
  run "Ports" with target = host, ports = "1-1024"
  if Ports.open > 0 {
    run "Grab the banner" with target = host
  }
}
```

**Log** keeps everything a run said, oldest first, stamped with how far into
the run each line was. That is where `print` goes, and it is the only place
every pass of one survives: the trail against a step keeps the latest pass,
so a `print` inside a loop would otherwise show only its last time round.
Failures and the start and end of the run are in there too, so it reads as a
record of what happened.

The list is any expression: a column of a run (`Sweep.host`), an array out of
an answer (`Health.json.hosts`), or a single value, which counts as a list of
one. An item that is an object keeps its shape, so the body can ask it for a
field:

```text
for each slide in Catalogue.json.slideshow.slides {
  run "Echo" with target = "https://example.com/" + slide.id
  print "sent " + slide.title
}
```

The name belongs to the walk, not to the workspace: it is gone when the
workflow finishes, and a walk inside a walk may reuse a name without the outer
one losing its place. Every step inside a loop says how many passes it did.

`with` sets the tool's own fields, by their names, each to whatever its
expression comes to at that moment. Completion offers the names once the line
says which tool it is. A field that tool does not have, or a value that cannot
be worked out, stops the workflow and says which one: running it as it
happened to be left would be doing something other than what was asked. The
tool keeps what it was last run with, so its form and its results agree
afterwards.

A `while` goes round for as long as its condition holds, which is what `repeat`
cannot say: waiting for something to come up, or draining a queue. It cannot
run away with the window — a workflow that asks for more than a million
operations is given up on where it stands, and the trail says so.

## Expressions

The same small language works inside `{{ }}` in a document, in a workflow's
conditions, and on the right hand side of `set`.

Name a run by whatever you called it, or by its tool:

```
Sweep.up          the run called Sweep
ipscan.rows       the only IP scan in this workspace
"Port scan".open  quotes, if the name has a space in it
tool("Port scan") same thing, spelled out
```

Every run answers to `rows`, `up`, `down`, `warn`, `target`, `state`, `ok`,
`elapsed` and `name`. It also answers to any of its columns by name
(`Sweep.rtt`, `Sweep.vendor`), which gives you a list with one entry per row,
and to any figure from its summary (`Uplink.loss`, `Sweep.scanned`).

Columns are text with units in them. The arithmetic reads the number out, so
`Sweep.rtt.avg()` averages `"12.4 ms"` and skips rows that never answered.

Functions, which you can also write as methods (`avg(x)` and `x.avg()` are the
same thing):

```
avg  min  max  sum  count  median  p95  percentile
round  floor  ceil  abs  fixed  percent
first  last  join  text  upper  lower  number
exists  tool  tools
```

Plus `if … then … else …`, the usual arithmetic and comparisons, and `&&`,
`||`, `!` (or `not`). No loops, no definitions, no side effects.

```
{{ if Sweep.up == 0 then "nothing answered" else Sweep.up + " hosts" }}
{{ fixed(percent(Sweep.up / Sweep.rows), 1) }}
{{ Sweep.host.join(", ") }}
```

## Variables

Variables belong to the workspace, not to any run in it, and everything in the
workspace can read them by name. They come in two flavours: plain text, and
formulas. A formula holds an expression that gets evaluated every time it's
read, so `gateway = Sweep.host.first()` follows the sweep, while the same thing
stored as text is whatever the sweep said on the day you wrote it down.

The Variables page lists them. Each row is the name, what it currently comes
to, and a second line with the formula on the left and everything else known
about it on the right:

```
gateway     10.0.0.1
            = Sweep.host.first()          text · read by Office LAN

hosts_up    10                            set by Nightly check · 8 min ago

subnet      10.0.0.0/24
            the network this is about     read by gateway
```

Click the name to rename, the value to retype it, the second line to write a
note about what it's for. Hovering a row brings up buttons to switch it between
text and formula, copy the value, copy it as `{{ a reference }}`, duplicate it
and remove it. The text is kept when you switch, so an expression you typed as
text starts working the moment you flip it. A formula that can't be evaluated
shows the reason in red where its answer would go. One that refers to itself,
directly or in a circle, comes back empty instead of hanging.

"Read by" is worked out from the documents, workflows and other formulas that
mention the name, so a variable nothing uses says so. "Reads" is the same list
the other way round.

### Workflows write them

A `set` step evaluates an expression when the step is reached and keeps the
answer:

```text
run "Sweep"
set hosts_up = Sweep.up
run "Ports on the NAS" if hosts_up > 0
```

None of that has to be typed. The name comes from the variables you already
have, and the value is picked the same way a condition is: which run,
which figure, and how to reduce a column of many rows to one value (`as it is`,
`avg`, `max`, `count`, `first`, and so on). The step shows what it would keep
while you're building it:

```
Set [hosts_up] = [Sweep] [up] [as it is]  → 10
```

Anything too involved for those controls you type instead, and it's marked *as
written*. When the workflow runs, the trail against that step says what it set,
and the variable remembers which workflow wrote it and when.

Conditions can be about a variable as easily as about a run. Pick a variable as
the subject and the "which figure" control disappears, since a variable is
already a value: `if [hosts_up] [is more than] [0]`.

If a run and a variable share a name, the run wins.

## Keyboard

Written the macOS way. On Windows and Linux every ⌘ is Ctrl.

| Where | Keys |
| --- | --- |
| Adding | `⌘K` command bar, `⌘1`–`⌘9` add that tool directly |
| Workspaces | `⌘T` new, `⌘⇧W` close, `⌘⇧[` / `⌘⇧]` previous/next |
| Tools | `⌘[` / `⌘]` previous/next, `⌘W` close the tab (tool stays) |
| A run | `⌘R` run, `⌘.` stop, `⌘E` its form, `⌘I` rename, `⌘G` big graph |
| Layout | `⌘B` side bar, `⌘J` output panel, `⇧⌘O` workspaces, `⇧⌘E` tools |
| Results | `↑↓` select, `pgup`/`pgdn` page, `⌘↑`/`⌘↓` first/last, `⌘F` filter, `⌘⇧N` note, `⏎` send onward |
| Anywhere | `⌘,` settings, `⌘⇧M` notifications, `⌘D` light/dark, `esc` back out, `⌘Q` quit |

## Permissions

Most of it needs nothing special. The exceptions are where the OS says so:

| | macOS | Linux | Windows |
| --- | --- | --- | --- |
| ICMP (ping, traceroute, ICMP sweep) | works unprivileged | unprivileged if `net.ipv4.ping_group_range` allows it, otherwise root | needs Administrator, Windows has no unprivileged ICMP socket |
| TTL column | yes | yes | blank, a Windows datagram socket doesn't carry it |
| ARP sweep and hardware addresses | real ARP over BPF with ChmodBPF installed, otherwise the neighbour table | neighbour table from `/proc/net/arp` | neighbour table from `arp -a` |
| Everything else | yes | yes | yes |

MAC vendors are looked up in a table of ~58,000 prefixes compiled into the
binary, so it's instant, works offline, and doesn't tell anyone what you're
scanning. The names come from the IEEE registries, which are authoritative but
publish a block as "IEEE Registration Authority" once they've subdivided it, so
several thousand small vendors' devices would otherwise be reported by the name
of their registrar. Those placeholders are dropped and the finer assignments
Wireshark maintains are merged in, which is what lets those devices be named.
`packaging/oui/build-oui.py` rebuilds the table and explains the rest.

Addresses a device made up for itself are reported as `randomised` rather than
guessed at, and a block whose registrant asked the IEEE not to list them reads
as `Private`, which is all anyone knows about it.

The download tool will use [yt-dlp](https://github.com/yt-dlp/yt-dlp) and
[gallery-dl](https://github.com/mikf/gallery-dl) if they're on your `PATH`, and
mentions it in the log if they aren't. Neither is bundled or required.

## Building

```
cargo build --release
cargo test
```

Rust 1.85 or newer. On macOS you'll want the Metal toolchain GPUI compiles
shaders with:

```
xcodebuild -downloadComponent MetalToolchain
```

On Debian and Ubuntu, the libraries GPUI links against:

```
sudo apt install libasound2-dev libfontconfig-dev libwayland-dev \
    libx11-xcb-dev libxcb1-dev libxcb-dri3-dev libxkbcommon-x11-dev \
    libssl-dev libzstd-dev libvulkan-dev make cmake clang
```

On Windows, just the MSVC toolchain.

The tests don't touch the network beyond this machine. They cover target and
port parsing, the certificate transparency aggregation, the vendor database,
the tool contract, the expression language, the workflow reader, writer and
machine, and a real port scan and ping against loopback.

Packaging lives in `packaging/`: `icon/make-icons.py` draws the icon and writes
out every format the three platforms want, `macos/bundle.sh` wraps a binary in
a `.app`, `windows/ntls.iss` is the Inno Setup script, and `linux/` has the
`.desktop` file and the tarball's installer. `.github/workflows/build.yml`
drives all of it: a test job, a build job per platform on a GitHub-hosted
runner of that platform, and a release job that collects what they made.

Each platform is built on its own machine because it has to be. macOS needs
Apple hardware, since GPUI compiles Metal shaders with `xcrun metal` and the
SDK is licensed to Apple machines. Windows needs Windows, for MSVC and Inno
Setup. GitHub hands out runners for all of them, including arm64 Linux and
Windows, which is the whole reason the builds live there.

## Adding a tool

The UI doesn't know what pinging or port scanning is. It builds a form out of
whatever fields a tool declares and a table out of whatever columns it
declares, so a new tool is one file plus one line in the registry.

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

A run emits events (a row, a log line, a summary figure, progress, a numeric
sample) and the UI turns those into a table, an output panel, a stat bar, a
progress bar and a live graph by itself. Write the impl in `src/tools/`, add a
line to `register`, and the picker, command bar, form, validation, hand-offs,
saving, resuming, CSV export and expression language all come with it.

## Licence

Apache-2.0, see [LICENSE](LICENSE).
