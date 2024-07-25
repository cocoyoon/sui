// Copyright (c) Mysten Labs, Inc.
// SPDX-License-Identifier: Apache-2.0

#[cfg(feature = "pg_integration")]
mod tests {
    use std::{path::PathBuf, time::Duration};

    use sui_graphql_rpc::{
        config::{ConnectionConfig, ServiceConfig},
        test_infra::cluster::{start_graphql_server_with_fn_rpc, wait_for_graphql_server},
    };
    use sui_graphql_rpc_client::simple_client::SimpleClient;
    use sui_json_rpc_types::{ObjectChange, SuiTransactionBlockResponse};
    use sui_move_build::BuildConfig;
    use sui_types::{
        base_types::{ObjectID, SequenceNumber},
        digests::ObjectDigest,
        move_package::UpgradePolicy,
        object::Owner,
        programmable_transaction_builder::ProgrammableTransactionBuilder,
        transaction::{CallArg, ObjectArg, TransactionData},
        Identifier, SUI_FRAMEWORK_PACKAGE_ID,
    };
    use tokio::time::sleep;

    const DOT_MOVE_PKG: &str = "tests/dot_move/dot_move/";
    const DEMO_PKG: &str = "tests/dot_move/demo/";
    const DEMO_PKG_V2: &str = "tests/dot_move/demo_v2/";
    const DEMO_PKG_V3: &str = "tests/dot_move/demo_v3/";

    #[derive(Clone, Debug)]
    struct UpgradeCap(ObjectID, SequenceNumber, ObjectDigest);

    #[tokio::test]
    async fn test_dot_move_e2e() {
        let network_cluster = sui_graphql_rpc::test_infra::cluster::start_network_cluster(
            ConnectionConfig::ci_integration_test_cfg(),
            None,
        )
        .await;

        let external_network_chain_id = network_cluster
            .validator_fullnode_handle
            .fullnode_handle
            .sui_client
            .read_api()
            .get_chain_identifier()
            .await
            .unwrap();

        println!("External chain id: {:?}", external_network_chain_id);

        // // publish the dot move package in the internal resolution cluster.
        let (pkg_id, registry_id) = publish_dot_move_package(&network_cluster).await;

        let (v1, v2, v3) = publish_demo_pkg(&network_cluster).await;

        let name = "app@org".to_string();

        // Register the package: First, for the "base" chain state.
        register_pkg(
            &network_cluster,
            pkg_id,
            registry_id,
            v1.clone(),
            name.clone(),
            None,
        )
        .await;

        // Register the package for the external resolver.
        register_pkg(
            &network_cluster,
            pkg_id,
            registry_id,
            v1,
            name.clone(),
            Some(external_network_chain_id.clone()),
        )
        .await;

        // Initialize the internal and external clients of GraphQL.

        // The first cluster uses internal resolution (mimics mainnet, does not rely on external chain).
        let internal_client = init_dot_move_gql(
            8000,
            9184,
            ServiceConfig::dot_move_test_defaults(
                false,
                None,
                Some(pkg_id.into()),
                Some(registry_id.0),
                None,
            ),
        )
        .await;

        // Wait for the transactions to be committed and indexed
        sleep(Duration::from_secs(5)).await;

        // We craft a big query, which we'll use to test both the internal and the external resolution.
        // Same query is used across both nodes, since we're testing on top of the same data, just with a different
        // lookup approach.
        let query = format!(
            r#"{{ valid_latest: {}, v1: {}, v2: {}, v3: {}, v4: {} }}"#,
            name_query(&name),
            name_query(&format!("{}{}", &name, "/v1")),
            name_query(&format!("{}{}", &name, "/v2")),
            name_query(&format!("{}{}", &name, "/v3")),
            name_query(&format!("{}{}", &name, "/v4")),
        );

        let internal_resolution = internal_client
            .execute(query.clone(), vec![])
            .await
            .unwrap();

        test_results(internal_resolution, &v1, &v2, &v3);

        println!("Tests have finished successfully now!");
    }

    fn test_results(query_result: serde_json::Value, v1: &ObjectID, v2: &ObjectID, v3: &ObjectID) {
        assert_eq!(
            query_result["data"]["valid_latest"]["address"]
                .as_str()
                .unwrap(),
            v3.to_string()
        );

        assert_eq!(
            query_result["data"]["v1"]["address"].as_str().unwrap(),
            v1.to_string()
        );

        assert_eq!(
            query_result["data"]["v2"]["address"].as_str().unwrap(),
            v2.to_string()
        );

        assert_eq!(
            query_result["data"]["v3"]["address"].as_str().unwrap(),
            v3.to_string()
        );

        assert!(query_result["data"]["v4"].is_null());
    }

    async fn init_dot_move_gql(
        gql_port: u16,
        prom_port: u16,
        config: ServiceConfig,
    ) -> SimpleClient {
        let cfg = ConnectionConfig::ci_integration_test_cfg_with_db_name(
            "sui_indexer".to_string(),
            gql_port,
            prom_port,
        );

        let _gql_handle =
            start_graphql_server_with_fn_rpc(cfg.clone(), None, None, Some(config)).await;

        let server_url = format!("http://{}:{}/", cfg.host(), cfg.port());

        // Starts graphql client
        let client = SimpleClient::new(server_url);
        wait_for_graphql_server(&client).await;

        client
    }

    async fn register_pkg(
        cluster: &sui_graphql_rpc::test_infra::cluster::NetworkCluster,
        dot_move_package_id: ObjectID,
        registry_id: (ObjectID, SequenceNumber),
        package_id: ObjectID,
        name: String,
        chain_id: Option<String>,
    ) {
        let is_network_call = chain_id.is_some();
        let function = if is_network_call {
            "set_network"
        } else {
            "add_record"
        };

        let mut args = vec![
            CallArg::Object(ObjectArg::SharedObject {
                id: registry_id.0,
                initial_shared_version: registry_id.1,
                mutable: true,
            }),
            CallArg::from(&name.as_bytes().to_vec()),
            CallArg::Pure(bcs::to_bytes(&package_id).unwrap()),
        ];

        if let Some(ref chain_id) = chain_id {
            args.push(CallArg::from(&chain_id.as_bytes().to_vec()));
        };

        let tx = cluster
            .validator_fullnode_handle
            .test_transaction_builder()
            .await
            .move_call(dot_move_package_id, "dotmove", function, args)
            .build();

        execute_tx(cluster, tx).await;

        println!("Added record successfully: {:?}", (name, chain_id));
    }

    // Publishes the Demo PKG, upgrades it twice and returns v1, v2 and v3 package ids.
    async fn publish_demo_pkg(
        cluster: &sui_graphql_rpc::test_infra::cluster::NetworkCluster,
    ) -> (ObjectID, ObjectID, ObjectID) {
        let tx = cluster
            .validator_fullnode_handle
            .test_transaction_builder()
            .await
            .publish(PathBuf::from(DEMO_PKG))
            .build();

        let executed = execute_tx(cluster, tx).await;
        let object_changes = executed.object_changes.unwrap();

        let v1 = object_changes
            .iter()
            .find_map(|object| {
                if let ObjectChange::Published { package_id, .. } = object {
                    Some(*package_id)
                } else {
                    None
                }
            })
            .unwrap();

        let upgrade_cap = object_changes
            .iter()
            .find_map(|object| {
                if let ObjectChange::Created {
                    object_id,
                    object_type,
                    digest,
                    version,
                    ..
                } = object
                {
                    if object_type.module.as_str() == "package"
                        && object_type.name.as_str() == "UpgradeCap"
                    {
                        Some(UpgradeCap(*object_id, *version, *digest))
                    } else {
                        None
                    }
                } else {
                    None
                }
            })
            .unwrap();

        let (v2, upgrade_cap) = upgrade_pkg(cluster, DEMO_PKG_V2, upgrade_cap, v1).await;
        let (v3, _) = upgrade_pkg(cluster, DEMO_PKG_V3, upgrade_cap, v2).await;

        (v1, v2, v3)
    }

    async fn upgrade_pkg(
        cluster: &sui_graphql_rpc::test_infra::cluster::NetworkCluster,
        package_path: &str,
        upgrade_cap: UpgradeCap,
        current_package_object_id: ObjectID,
    ) -> (ObjectID, UpgradeCap) {
        // build the package upgrade to V2.
        let mut builder = ProgrammableTransactionBuilder::new();

        let compiled_package = BuildConfig::new_for_testing()
            .build(PathBuf::from(package_path))
            .unwrap();
        let digest = compiled_package.get_package_digest(false);
        let modules = compiled_package.get_package_bytes(false);
        let dependencies = compiled_package.get_dependency_original_package_ids();

        let cap = builder
            .obj(ObjectArg::ImmOrOwnedObject((
                upgrade_cap.0,
                upgrade_cap.1,
                upgrade_cap.2,
            )))
            .unwrap();

        let policy = builder.pure(UpgradePolicy::Compatible as u8).unwrap();

        let digest = builder.pure(digest.to_vec()).unwrap();

        let ticket = builder.programmable_move_call(
            SUI_FRAMEWORK_PACKAGE_ID,
            Identifier::new("package").unwrap(),
            Identifier::new("authorize_upgrade").unwrap(),
            vec![],
            vec![cap, policy, digest],
        );

        let receipt = builder.upgrade(current_package_object_id, ticket, dependencies, modules);

        builder.programmable_move_call(
            SUI_FRAMEWORK_PACKAGE_ID,
            Identifier::new("package").unwrap(),
            Identifier::new("commit_upgrade").unwrap(),
            vec![],
            vec![cap, receipt],
        );

        let tx = cluster
            .validator_fullnode_handle
            .test_transaction_builder()
            .await
            .programmable(builder.finish())
            .build();

        let upgraded = execute_tx(cluster, tx).await;

        let object_changes = upgraded.object_changes.unwrap();

        let pkg_id = object_changes
            .iter()
            .find_map(|object| {
                if let ObjectChange::Published { package_id, .. } = object {
                    Some(*package_id)
                } else {
                    None
                }
            })
            .unwrap();

        let upgrade_cap = object_changes
            .iter()
            .find_map(|object| {
                if let ObjectChange::Mutated {
                    object_id,
                    object_type,
                    digest,
                    version,
                    ..
                } = object
                {
                    if object_type.module.as_str() == "package"
                        && object_type.name.as_str() == "UpgradeCap"
                    {
                        Some(UpgradeCap(*object_id, *version, *digest))
                    } else {
                        None
                    }
                } else {
                    None
                }
            })
            .unwrap();

        (pkg_id, upgrade_cap)
    }

    async fn publish_dot_move_package(
        cluster: &sui_graphql_rpc::test_infra::cluster::NetworkCluster,
    ) -> (ObjectID, (ObjectID, SequenceNumber)) {
        let package_path = PathBuf::from(DOT_MOVE_PKG);
        let tx = cluster
            .validator_fullnode_handle
            .test_transaction_builder()
            .await
            .publish(package_path)
            .build();

        let sig = cluster
            .validator_fullnode_handle
            .wallet
            .sign_transaction(&tx);

        let executed = cluster
            .validator_fullnode_handle
            .execute_transaction(sig)
            .await;

        let (mut pkg_id, mut obj_id) = (None, None);

        for object in executed.object_changes.unwrap() {
            match object {
                ObjectChange::Published { package_id, .. } => {
                    pkg_id = Some(package_id);
                }
                ObjectChange::Created {
                    object_id,
                    object_type,
                    owner,
                    ..
                } => {
                    if object_type.module.as_str() == "dotmove"
                        && object_type.name.as_str() == "AppRegistry"
                    {
                        let initial_shared_version = match owner {
                            Owner::Shared {
                                initial_shared_version,
                            } => initial_shared_version,
                            _ => panic!("AppRegistry should be shared"),
                        };

                        if !owner.is_shared() {
                            panic!("AppRegistry should be shared");
                        };

                        obj_id = Some((object_id, initial_shared_version));
                    }
                }
                _ => {}
            }
        }

        (pkg_id.unwrap(), obj_id.unwrap())
    }

    fn name_query(name: &str) -> String {
        format!(r#"packageByName(name: "{}") {{ address, version }}"#, name)
    }

    async fn execute_tx(
        cluster: &sui_graphql_rpc::test_infra::cluster::NetworkCluster,
        tx: TransactionData,
    ) -> SuiTransactionBlockResponse {
        let sig = cluster
            .validator_fullnode_handle
            .wallet
            .sign_transaction(&tx);

        let executed = cluster
            .validator_fullnode_handle
            .execute_transaction(sig)
            .await;

        executed
    }
}
