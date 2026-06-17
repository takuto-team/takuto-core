// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! `takuto` binary entrypoint. Parses the CLI and dispatches: subcommands go to
//! [`commands`], the default (no subcommand) path boots the web server via
//! [`server::run_server`].

mod cli;
mod commands;
mod server;

use std::process::ExitCode;

use clap::Parser;

use cli::{Cli, Commands, KeysAction};

fn main() -> ExitCode {
    let cli = Cli::parse();

    match &cli.command {
        Some(Commands::DockerHooks { phase }) => commands::run_docker_hooks(&cli.config, *phase),
        Some(Commands::Preflight { strict }) => commands::run_preflight(&cli.config, *strict),
        Some(Commands::GithubAppToken) => {
            match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt.block_on(commands::run_github_app_token(&cli.config)),
                Err(e) => {
                    eprintln!("Failed to start async runtime: {e}");
                    ExitCode::FAILURE
                }
            }
        }
        Some(Commands::Keys {
            action: KeysAction::Reset { yes_i_am_sure },
        }) => commands::run_keys_reset(&cli.config, *yes_i_am_sure),
        Some(Commands::Provisioning { action }) => commands::run_provisioning(&cli.config, action),
        Some(Commands::EgressHosts) => commands::run_egress_hosts(&cli.config),
        None => match tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => match rt.block_on(server::run_server(&cli)) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("Takuto error: {e}");
                    ExitCode::FAILURE
                }
            },
            Err(e) => {
                eprintln!("Failed to start async runtime: {e}");
                ExitCode::FAILURE
            }
        },
    }
}
