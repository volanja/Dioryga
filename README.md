# Dioryga

![coverage](docs/badges/coverage.svg)

Dioryga is a web application for recording the servers and network equipment used in system build and operations projects, from purchase to disposal.

Dioryga is under development and has not been released yet. The web screens and the navigation between them are being reworked and may change significantly. Upgrading to a new version may require recreating an existing database.

This README is written in English, but development is done in Japanese. The web screens are available in both Japanese and English.

## About the name

The name comes from διώρυγα, Greek for "canal", and is pronounced *dee-OH-ree-ga*. Unlike a natural river, a canal is a waterway that people deliberately design, build and maintain. Servers and network equipment are likewise planned, procured and managed until they are retired, and the name was chosen for that shared trait.

## Features

- Device register: record physical machines, virtual machines and containers. Changes to a device's project, mounting position, IP addresses and similar attributes do not overwrite earlier values; they are kept as history
- Racks: record where devices are mounted and show them as rack diagrams
- Network: manage subnets, IP addresses and device interfaces (VLANs, IP addresses, bonding)
- Software: import an SBOM for each device and search its components across devices
- Costs and contracts: manage purchase records, fixed assets, maintenance contracts and recurring costs
- Milestones: record planned and actual dates for service start, renewal, maintenance and end, and show delays against the plan
- Change management tickets: raise tickets for adding, relocating or removing devices, and carry them out after approval
- Warehouses: manage, across projects, where devices and parts not assigned to any project are kept
- Shared catalog: share definitions of vendors, models, configurations, parts, cables, software and VLANs across all projects
- Users and projects: add members to each project and assign them roles (Administrator, Operator, Approver, Viewer)

Dioryga runs as a single executable and works with either SQLite or PostgreSQL.

The definitions of all database tables and columns, with ER diagrams, are in [`docs/schema/`](docs/schema/README.md).

## Installation

Dioryga is currently available only by building from source.

Install Rust with [rustup](https://rustup.rs/) first. The Rust version used for the build is pinned in the repository, and rustup installs it during the build if it is missing.

```bash
git clone https://github.com/volanja/Dioryga.git
cd Dioryga
cargo build --release --locked
```

The executable is created at `target/release/dioryga` (`dioryga.exe` on Windows). Dioryga has been tested on macOS and Windows.

## Usage

### Starting the server

```bash
dioryga
```

Without a configuration file, Dioryga starts with the default settings: it listens on `127.0.0.1:8080` and stores data in `dioryga.db` (SQLite) in the current directory.

### Creating the first administrator

On first start, Dioryga prints a setup URL and a setup token to the console, once only. Open the URL (by default `http://127.0.0.1:8080/setup`), enter the token and create the System Admin, the administrator of the whole system.

### Starting a project

The System Admin manages users and projects, and cannot see the data inside projects. To start registering devices, set things up in this order:

1. The System Admin creates users and a project
2. The System Admin assigns administrators to the project
3. A project administrator adds members and assigns their roles

After logging in, members can register devices and do other work in the projects they belong to.

### Configuration

Settings are loaded in the following order, and later sources override earlier ones:

1. Defaults
2. The configuration file `dioryga.toml` (use `-c` to specify another path)
3. Environment variables `DIORYGA_*` (separate nested keys with `__`, as in `DIORYGA_DATABASE__URL`)

An example configuration file is in [`dioryga.toml.example`](dioryga.toml.example). To use PostgreSQL, specify the connection as a URL:

```toml
[database]
url = "postgres://dioryga:password@localhost:5432/dioryga"
```

## Commands

| Command | Description |
|---|---|
| `dioryga`<br>`dioryga serve` | Start the server |
| `dioryga migrate` | Create or update the database tables for this version of Dioryga |
| `dioryga admin create --username <name>` | Create a System Admin |
| `dioryga admin reset-password --username <name>` | Issue a temporary password to a user |
| `dioryga licenses` | Print the license notices for Dioryga and its dependencies |

Every command accepts `-c <path>` to specify the configuration file.

`migrate` creates the tables the first time, and after an upgrade brings the table definitions in line with the new version. The server does the same automatically on startup, so you normally do not need to run it. Use it when automatic migration is turned off in the configuration (`database.auto_migrate = false`), or when you want to update the database before starting the server.

`admin create` is for creating a System Admin when the setup screen cannot be used. It asks for the display name and password interactively. To run it non-interactively, pass the display name with `--name` and add `--password-stdin` to read the password from standard input. The password is not accepted as a command-line argument, because it would be left in plain text in the shell history and the process list.

```bash
printf '%s\n' "$PASSWORD" | dioryga admin create --username admin --name Administrator --password-stdin
```

`admin reset-password` issues a temporary password, prints it to the console and ends all of the user's active sessions. The user is asked to change the password at the next login.

## License

Copyright (c) 2026 volanja

Dioryga is dual-licensed under [MIT](LICENSE-MIT) and [Apache-2.0](LICENSE-APACHE). You may choose either license.

Unless explicitly stated otherwise, any contribution to this repository is licensed under the dual license above (Apache-2.0, Section 5).

License notices for the open source software Dioryga depends on are embedded in the executable. Run `dioryga licenses` to print them in full.
