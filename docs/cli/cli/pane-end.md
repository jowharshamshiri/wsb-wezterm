# `wsb-wezterm cli pane-end`

{{since('nightly')}}

*Run `wsb-wezterm cli pane-end --help` to see more help*

Prints how a pane ended, as JSON: why the multiplexer closed it, the client
that asked, and how the program in the pane ended. A program in a pane is
told its pane is closing only by a hangup (SIGHUP), which says nothing of
why; it can ask here, as can whoever finds the pane gone later.

The pane is the one given by `--pane-id`, which has no default: the pane is
usually gone by the time anyone asks.

```console
$ wsb-wezterm cli pane-end --pane-id 3
{"pane_id":3,"closed":{"why":"tab","client":{"hostname":"box","username":"me","pid":4242,"lineage":[{"pid":4242,"argv":["wsb-wezterm-gui","connect","unix"]},{"pid":1830,"argv":["/usr/lib/systemd/systemd","--user"]}]},"signal":null,"at_ms":1794790000123},"program":{"pid":5120,"code":null,"signal":1,"at_ms":1794790000140}}
```

`closed` is `null` when the multiplexer did not close the pane, and its
program ended by itself. Otherwise:

| Field | |
|---|---|
| `why` | `pane`: a client closed the pane, such as with [kill-pane](kill-pane.md); `tab` or `window`: a client closed the pane's tab or window; `server_exiting`: the server was stopped by a signal (below) |
| `client` | The client that asked, by its machine, user and process. When the client runs on the server's machine, `lineage` is its command line and those of the programs that started it, nearest first, as they were when it asked; it is empty for a client elsewhere |
| `signal` | When the server was exiting, the signal that stopped it (`number`), the process that sent it (`sender`, where the system names one) and that process's `lineage` |
| `at_ms` | When the pane was closed, in milliseconds since the Unix epoch |

`program` is `null` while the program still runs (a program asking about its
own pane, as its hangup arrives, sees this). Once it has ended, it holds its
process id, its exit `code`, or the number of the `signal` that ended it, and
when the multiplexer saw it end.

A multiplexer keeps the ends of the last 1024 panes it closed or whose program
ended. For a pane it keeps no end of, because it still runs and is not
closing, or it ended before those, the command says so and exits with
status 3; any other failure, such as no multiplexer answering, exits with 1.

## A server that is stopped

A server sent SIGTERM, SIGINT or SIGHUP closes every pane before it exits: it
hangs up each pane's program, keeping as the pane's end that the server was
exiting, with the signal and its sender, and answers `pane-end` until every
program has ended, or for 10 seconds at most. It then exits with status 0.

The server takes over these signals before it listens, so any client that can
reach it stops it this way. A server stopped while it is still starting,
before its first pane is there, has no pane to close: it exits at once with
status 0 and keeps no end for that pane. A program it was starting in that
pane is hung up as the server's terminal goes, before it can ask how its pane
ended.

## Synopsis

```console
{% include "../../examples/cmd-synopsis-wsb-wezterm-cli-pane-end--help.txt" %}
```
