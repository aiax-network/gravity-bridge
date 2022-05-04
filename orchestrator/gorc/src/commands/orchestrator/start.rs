use crate::{application::APP, prelude::*};
use abscissa_core::{Clap, Command, Runnable};
use clarity::address::Address as EthAddress;
use gravity_utils::connection_prep::{
    check_delegate_addresses, check_for_eth, check_for_fee_denom, create_rpc_connections,
    wait_for_cosmos_node_ready,
};
use orchestrator::main_loop::{
    orchestrator_main_loop, ETH_ORACLE_LOOP_SPEED, ETH_SIGNER_LOOP_SPEED,
};
use relayer::main_loop::LOOP_SPEED as RELAYER_LOOP_SPEED;
use std::cmp::min;

/// Start the Orchestrator
#[derive(Command, Debug, Clap)]
pub struct StartCommand {
    #[clap(short, long)]
    cosmos_key: String,

    #[clap(short, long)]
    ethereum_key: String,

    #[clap(short, long)]
    orchestrator_only: bool,
}

impl Runnable for StartCommand {
    fn run(&self) {
        openssl_probe::init_ssl_cert_env_vars();

        let config = APP.config();
        let cosmos_prefix = config.cosmos.prefix.clone();

        let cosmos_key = config.load_deep_space_key(self.cosmos_key.clone());
        let cosmos_address = cosmos_key.to_address(&cosmos_prefix).unwrap();

        let ethereum_key = config.load_clarity_key(self.ethereum_key.clone());
        let ethereum_address = ethereum_key.to_public_key().unwrap();

        let contract_address: EthAddress = config
            .gravity
            .contract
            .parse()
            .expect("Could not parse gravity contract address");

        let fees_denom = config.gravity.fees_denom.clone();

        let timeout = min(
            min(ETH_SIGNER_LOOP_SPEED, ETH_ORACLE_LOOP_SPEED),
            RELAYER_LOOP_SPEED,
        );

        abscissa_tokio::run_with_actix(&APP, async {
            let connections = create_rpc_connections(
                cosmos_prefix,
                Some(config.cosmos.grpc.clone()),
                Some(config.ethereum.rpc.clone()),
                timeout,
            )
            .await;

            let mut grpc = connections.grpc.clone().unwrap();
            let contact = connections.contact.clone().unwrap();
            let web3 = connections.web3.clone().unwrap();

            info!("Starting Relayer + Oracle + Ethereum Signer");
            info!("Ethereum Address: {}", ethereum_address);
            info!("Cosmos Address {}", cosmos_address);

            // check if the cosmos node is syncing, if so wait for it
            // we can't move any steps above this because they may fail on an incorrect
            // historic chain state while syncing occurs
            wait_for_cosmos_node_ready(&contact).await;

            // foolproof
            // check that gravity contract is matching cosmos chain
            // without this check someone can join with wrong contract address and get stuck
            // added after I have broken testnet with wrong contract address
            extra_checks::check_gravity_id(&mut grpc, &web3, ethereum_address, contract_address).await;

            // check if the delegate addresses are correctly configured
            check_delegate_addresses(
                &mut grpc,
                ethereum_address,
                cosmos_address,
                &contact.get_prefix(),
            )
            .await;

            // check if we actually have the promised balance of tokens to pay fees
            check_for_fee_denom(&fees_denom, cosmos_address, &contact).await;
            check_for_eth(ethereum_address, &web3).await;

            let gas_price = config.cosmos.gas_price.as_tuple();

            orchestrator_main_loop(
                cosmos_key,
                ethereum_key,
                web3,
                contact,
                grpc,
                contract_address,
                gas_price,
                &config.metrics.listen_addr,
                config.ethereum.gas_price_multiplier,
                config.ethereum.blocks_to_search as u128,
                config.cosmos.gas_adjustment,
                self.orchestrator_only,
                config.cosmos.msg_batch_size,
            )
            .await;
        })
        .unwrap_or_else(|e| {
            status_err!("executor exited with error: {}", e);
            std::process::exit(1);
        });
    }
}

// Function is placed here instead of connection_prep.rs because it requires ethereum_gravity crate
// and ethereum_gravity crate depends on gravity_utils that contains connection_prep.rs
mod extra_checks {
    use crate::prelude::*;
    use cosmos_sdk_proto::gravity::query_client::QueryClient as GravityQueryClient;
    use clarity::address::Address as EthAddress;
    use cosmos_sdk_proto::gravity::ParamsRequest;
    use ethereum_gravity::utils::get_gravity_id;
    use tonic::transport::Channel;
    use web30::client::Web3;

    /// Checks that gravityId from eth contract matches the cosmos network one
    pub async fn check_gravity_id(
        client: &mut GravityQueryClient<Channel>,
        web3: &Web3,
        address: EthAddress,
        contract_address: EthAddress, 
    ) {
        let eth_gid = {
            let gid = get_gravity_id(contract_address, address, &web3).await.unwrap();
            gid.trim_end_matches('\0').to_owned()
        };
        let cosmos_gid = client.params(ParamsRequest{}).await.unwrap().into_inner().params.unwrap().gravity_id;
        if eth_gid != cosmos_gid {
            error!("Ethereum contract gravityId does not match Cosmos one");
            error!(
                "eth_gravity_id {:?} != cosmos_gravity_id {:?}",
                eth_gid, cosmos_gid,
            );
            std::process::exit(1);
        }
    }
}
