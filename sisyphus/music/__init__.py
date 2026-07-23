"""Opt-in music sidecar (hypothesis H2). Disabled unless explicitly invoked.

Nothing in this package runs on import: there is no network service, no
microphone capture, and no background daemon. Every stage is an explicit,
bounded, offline batch command driven by ``sisyphus.music_sidecar``.
"""
