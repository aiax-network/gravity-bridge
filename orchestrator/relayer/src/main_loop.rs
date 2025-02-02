use crate::{
    batch_relaying::relay_batches, find_latest_valset::find_latest_valset,
    logic_call_relaying::relay_logic_calls, valset_relaying::relay_valsets,
};
use clarity::address::Address as EthAddress;
use clarity::PrivateKey as EthPrivateKey;
use ethereum_gravity::utils::get_gravity_id;
use cosmos_sdk_proto::gravity::query_client::QueryClient as GravityQueryClient;
use std::time::Duration;
use tonic::transport::Channel;
use web30::client::Web3;

pub const LOOP_SPEED: Duration = Duration::from_secs(17);

/// This function contains the orchestrator primary loop, it is broken out of the main loop so that
/// it can be called in the test runner for easier orchestration of multi-node tests
#[allow(unused_variables)]
pub async fn relayer_main_loop(
    ethereum_key: EthPrivateKey,
    web3: Web3,
    grpc_client: GravityQueryClient<Channel>,
    gravity_contract_address: EthAddress,
    gas_multiplier: f32,
) {
    let mut grpc_client = grpc_client;
    loop {
        let (async_resp, _) = tokio::join!(
            async {
                let our_ethereum_address = ethereum_key.to_public_key().unwrap();
                let current_eth_valset =
                    find_latest_valset(&mut grpc_client, gravity_contract_address, &web3).await;
                if current_eth_valset.is_err() {
                    error!("Could not get current valset! {:?}", current_eth_valset);
                }
                let current_eth_valset = current_eth_valset.unwrap();

                let gravity_id =
                    get_gravity_id(gravity_contract_address, our_ethereum_address, &web3).await;
                if gravity_id.is_err() {
                    error!("Failed to get GravityID, check your Eth node");
                    return;
                }
                let gravity_id = gravity_id.unwrap();

                relay_valsets(
                    current_eth_valset.clone(),
                    ethereum_key,
                    &web3,
                    &mut grpc_client,
                    gravity_contract_address,
                    gravity_id.clone(),
                    LOOP_SPEED,
                )
                .await;

                relay_batches(
                    current_eth_valset.clone(),
                    ethereum_key,
                    &web3,
                    &mut grpc_client,
                    gravity_contract_address,
                    gravity_id.clone(),
                    LOOP_SPEED,
                    gas_multiplier,
                )
                .await;

                relay_logic_calls(
                    current_eth_valset,
                    ethereum_key,
                    &web3,
                    &mut grpc_client,
                    gravity_contract_address,
                    gravity_id.clone(),
                    LOOP_SPEED,
                    gas_multiplier,
                )
                .await;
            },
            tokio::time::sleep(LOOP_SPEED)
        );
    }
}
