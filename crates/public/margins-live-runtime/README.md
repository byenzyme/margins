# Margins live runtime

`margins-live-runtime` is the small seam between a Margins recording process
and a surface that controls it, such as the bb plugin or a future CLI daemon.

It defines two things: how to read the current bounded meeting state, and how
to ask the process to start, pause, resume, stop, or replace the visible
notepad against its last revision. It does not
choose a transport, open a window, discover a process, install software, or
implement audio capture. Those stay with the process and its adapters.

Today the desktop process implements this seam and exposes it through its
private loopback service. A CLI-owned runtime can implement the same trait
later without changing the bb-facing live contract.
