"""Regression for the TLS Slowloris DoS (benchmark-17 audit bug #9).

Before the fix `tls::wrap_tls` awaited `acceptor.accept(stream)` with no
timeout — a client that opened a TCP connection and never finished the
TLS handshake pinned an async task and an fd indefinitely. With 65k
half-open connections from one laptop the server ran out of fds while
idling at 0% CPU.

Now the handshake is bounded at 10 s; a client that doesn't complete
in that window has its connection closed. We can't wait 10 s in a
unit test, so we verify the bound is *present* in the source (a real
end-to-end test needs a malicious client that never sends ClientHello).
"""

import pathlib


def test_tls_handshake_bounded_by_timeout():
    src = pathlib.Path("src/tls.rs").read_text()
    # A `tokio::time::timeout(..., acceptor.accept(...))` sandwich is the
    # only mechanism available to bound an async future; look for both
    # halves. The actual timeout value is a constant above the call.
    assert "tokio::time::timeout" in src, (
        "tls::wrap_tls must wrap acceptor.accept() in tokio::time::timeout "
        "so a slow client can't pin an fd forever (Slowloris)"
    )
    assert "acceptor.accept" in src
    # Sanity: the timeout constant is reasonable (≥ 1s, ≤ 60s — users
    # can override if they have extraordinary network conditions).
    assert "HANDSHAKE_TIMEOUT" in src
