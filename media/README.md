# media

`ask-demo.gif` is the README demo: the same English question answered in
PowerShell, bash, and nushell, then the destructive gate refusing `--yes`.

The `.tape` files are [vhs](https://github.com/charmbracelet/vhs) scripts, one
per cut. The answers shown are real model output (codex, `gpt-5.6-luna`),
captured once with `ask --json` and replayed through a `kind = "command"`
fixture provider so recordings are deterministic — the fixture sleeps ~1.2 s
and prints the captured JSON.

The tapes assume a recording environment that is not checked in (paths under
`/home/<user>/augur-gif`): a demo directory, per-cut `rec-*/` dirs each holding
`payload.json`, `fixture.sh`, and a `config.toml` pointing augur at the
fixture, plus a release build of `ask` on `PATH`. Recreate that layout, adjust
the absolute paths in the tapes, then:

```console
vhs pwsh.tape && vhs bash.tape && vhs nu.tape && vhs gate.tape
ffmpeg -f concat -safe 0 -i list.txt -c:v libx264 -crf 20 -pix_fmt yuv420p ask-demo.mp4
ffmpeg -i ask-demo.mp4 -vf "fps=15,split[s0][s1];[s0]palettegen=stats_mode=diff[p];[s1][p]paletteuse=dither=bayer:bayer_scale=4" ask-demo.gif
```
