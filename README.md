# Prometheus Speedtest

This server application serves two endpoints for measuring network speed and performance.

| Endpoint     | Purpose                             |
| ------------ | ----------------------------------- |
| `/speedtest` | Speedtest for upload and download   |
| `/ping`      | Measure ping to different addresses |

All endpoints both support the Prometheus [Exposition format] (default) and JSON. The server is configured via a TOML configuration file. The default config can be obtained by running the binary with the `print-default-config` subcommand. All absent keys default to these values. The config file's location is supplied via the `--config <CONFIG>` option. Run with `--help` to see all options.

[Exposition format]: https://prometheus.io/docs/instrumenting/exposition_formats/#text-based-format
