use std::io::Read;
use std::ops::Mul;
use std::str::FromStr;

use cosmos_sdk_proto::cosmos::staking::v1beta1::{MsgDelegate, MsgUndelegate};
use cosmwasm_std::testing::{mock_env, mock_info, MockApi, MockStorage, MOCK_CONTRACT_ADDR};
use cosmwasm_std::{
    from_binary, to_binary, Addr, BankMsg, Coin, CosmosMsg, Decimal, Event, Order, OwnedDeps,
    Reply, ReplyOn, StdError, SubMsg, SubMsgResponse, Uint128, Uint64, WasmMsg,
};
use cw20::{Cw20ExecuteMsg, MinterResponse};
use cw20_base::msg::InstantiateMsg as Cw20InstantiateMsg;

use pfc_steak::hub::{
    Batch, CallbackMsg, ConfigResponse, ExecuteMsg, InstantiateMsg, PendingBatch, QueryMsg,
    ReceiveMsg, StateResponse, UnbondRequest, UnbondRequestsByBatchResponseItem,
    UnbondRequestsByUserResponseItem,
};

use crate::contract::{
    execute, instantiate, reply, REPLY_INSTANTIATE_TOKEN, REPLY_REGISTER_RECEIVED_COINS,
};
use crate::helpers::{parse_coin, parse_received_fund};
use crate::math::{
    compute_redelegations_for_rebalancing, compute_redelegations_for_removal,
    compute_target_delegation_from_mining_power, compute_undelegations,
};
use crate::state::State;
use crate::types::{Coins, Delegation, Redelegation, RewardWithdrawal, Undelegation};

use super::custom_querier::CustomQuerier;
use super::helpers::{mock_dependencies, mock_env_at_timestamp, query_helper};

//--------------------------------------------------------------------------------------------------
// Test setup
//--------------------------------------------------------------------------------------------------

fn setup_test() -> OwnedDeps<MockStorage, MockApi, CustomQuerier> {
    let mut deps = mock_dependencies();

    let res = instantiate(
        deps.as_mut(),
        mock_env_at_timestamp(10000),
        mock_info("deployer", &[]),
        InstantiateMsg {
            cw20_code_id: 69420,
            owner: "larry".to_string(),
            name: "Steak Token".to_string(),
            symbol: "STEAK".to_string(),
            denom: "uxyz".to_string(),
            fee_account_type: "Wallet".to_string(),
            fee_account: "the_fee_man".to_string(),
            fee_amount: Decimal::from_ratio(10_u128, 100_u128), //10%
            max_fee_amount: Decimal::from_ratio(20_u128, 100_u128), //20%
            decimals: 6,
            epoch_period: 259200,   // 3 * 24 * 60 * 60 = 3 days
            unbond_period: 1814400, // 21 * 24 * 60 * 60 = 21 days
            validators: vec![
                "alice".to_string(),
                "bob".to_string(),
                "charlie".to_string(),
            ],
            label: None,
            marketing: None,
        },
    )
    .unwrap();

    assert_eq!(res.messages.len(), 1);
    assert_eq!(
        res.messages[0],
        SubMsg::reply_on_success(
            CosmosMsg::Wasm(WasmMsg::Instantiate {
                admin: Some("larry".to_string()),
                code_id: 69420,
                msg: to_binary(&Cw20InstantiateMsg {
                    name: "Steak Token".to_string(),
                    symbol: "STEAK".to_string(),
                    decimals: 6,
                    initial_balances: vec![],
                    mint: Some(MinterResponse {
                        minter: MOCK_CONTRACT_ADDR.to_string(),
                        cap: None
                    }),
                    marketing: None,
                })
                .unwrap(),
                funds: vec![],
                label: "steak_token".to_string(),
            }),
            REPLY_INSTANTIATE_TOKEN
        )
    );

    let event = Event::new("instantiate")
        .add_attribute("code_id", "69420")
        .add_attribute("_contract_address", "steak_token");

    let res = reply(
        deps.as_mut(),
        mock_env_at_timestamp(10000),
        Reply {
            id: REPLY_INSTANTIATE_TOKEN,
            result: cosmwasm_std::SubMsgResult::Ok(SubMsgResponse {
                events: vec![event],
                data: None,
            }),
        },
    )
    .unwrap();

    assert_eq!(res.messages.len(), 0);

    deps.querier.set_cw20_total_supply("steak_token", 0);
    deps
}

fn setup_test_fee_split() -> OwnedDeps<MockStorage, MockApi, CustomQuerier> {
    let mut deps = mock_dependencies();

    let res = instantiate(
        deps.as_mut(),
        mock_env_at_timestamp(10000),
        mock_info("deployer", &[]),
        InstantiateMsg {
            cw20_code_id: 69420,
            owner: "larry".to_string(),
            name: "Steak Token".to_string(),
            symbol: "STEAK".to_string(),
            denom: "uxyz".to_string(),
            fee_account_type: "FeeSplit".to_string(),
            fee_account: "fee_split_contract".to_string(),
            fee_amount: Decimal::from_ratio(10_u128, 100_u128), //10%
            max_fee_amount: Decimal::from_ratio(20_u128, 100_u128), //20%
            decimals: 6,
            epoch_period: 259200,   // 3 * 24 * 60 * 60 = 3 days
            unbond_period: 1814400, // 21 * 24 * 60 * 60 = 21 days
            validators: vec![
                "alice".to_string(),
                "bob".to_string(),
                "charlie".to_string(),
            ],
            label: None,
            marketing: None,
        },
    )
    .unwrap();

    assert_eq!(res.messages.len(), 1);
    assert_eq!(
        res.messages[0],
        SubMsg::reply_on_success(
            CosmosMsg::Wasm(WasmMsg::Instantiate {
                admin: Some("larry".to_string()),
                code_id: 69420,
                msg: to_binary(&Cw20InstantiateMsg {
                    name: "Steak Token".to_string(),
                    symbol: "STEAK".to_string(),
                    decimals: 6,
                    initial_balances: vec![],
                    mint: Some(MinterResponse {
                        minter: MOCK_CONTRACT_ADDR.to_string(),
                        cap: None
                    }),
                    marketing: None,
                })
                .unwrap(),
                funds: vec![],
                label: "steak_token".to_string(),
            }),
            REPLY_INSTANTIATE_TOKEN
        )
    );

    let event = Event::new("instantiate")
        .add_attribute("code_id", "69420")
        .add_attribute("_contract_address", "steak_token");

    let res = reply(
        deps.as_mut(),
        mock_env_at_timestamp(10000),
        Reply {
            id: REPLY_INSTANTIATE_TOKEN,
            result: cosmwasm_std::SubMsgResult::Ok(SubMsgResponse {
                events: vec![event],
                data: None,
            }),
        },
    )
    .unwrap();

    assert_eq!(res.messages.len(), 0);

    deps.querier.set_cw20_total_supply("steak_token", 0);
    deps
}

//--------------------------------------------------------------------------------------------------
// Execution
//--------------------------------------------------------------------------------------------------

#[test]
fn proper_instantiation() {
    let deps = setup_test();

    let res: ConfigResponse = query_helper(deps.as_ref(), QueryMsg::Config {});
    assert_eq!(
        res,
        ConfigResponse {
            owner: "larry".to_string(),
            new_owner: None,
            steak_token: "steak_token".to_string(),
            epoch_period: 259200,
            unbond_period: 1814400,
            denom: "uxyz".to_string(),
            fee_type: "Wallet".to_string(),
            fee_account: "the_fee_man".to_string(),
            fee_rate: Decimal::from_ratio(10_u128, 100_u128),
            max_fee_rate: Decimal::from_ratio(20_u128, 100_u128),
            validators: vec![
                "alice".to_string(),
                "bob".to_string(),
                "charlie".to_string()
            ]
        }
    );

    let res: StateResponse = query_helper(deps.as_ref(), QueryMsg::State {});
    assert_eq!(
        res,
        StateResponse {
            total_usteak: Uint128::zero(),
            total_native: Uint128::zero(),
            exchange_rate: Decimal::one(),
            unlocked_coins: vec![],
        },
    );

    let res: PendingBatch = query_helper(deps.as_ref(), QueryMsg::PendingBatch {});
    assert_eq!(
        res,
        PendingBatch {
            id: 1,
            usteak_to_burn: Uint128::zero(),
            est_unbond_start_time: 269200, // 10,000 + 259,200
        },
    );
    let deps_fee_split = setup_test_fee_split();

    let res_fee_split: ConfigResponse = query_helper(deps_fee_split.as_ref(), QueryMsg::Config {});
    assert_eq!(
        res_fee_split,
        ConfigResponse {
            owner: "larry".to_string(),
            new_owner: None,
            steak_token: "steak_token".to_string(),
            epoch_period: 259200,
            unbond_period: 1814400,
            denom: "uxyz".to_string(),
            fee_type: "FeeSplit".to_string(),
            fee_account: "fee_split_contract".to_string(),
            fee_rate: Decimal::from_ratio(10_u128, 100_u128),
            max_fee_rate: Decimal::from_ratio(20_u128, 100_u128),
            validators: vec![
                "alice".to_string(),
                "bob".to_string(),
                "charlie".to_string()
            ]
        }
    );
}

#[test]
fn bonding() {
    let mut deps = setup_test();
    let env = mock_env();
    // Bond when no delegation has been made
    // In this case, the full deposit simply goes to the first validator
    let res = execute(
        deps.as_mut(),
        env.clone(),
        mock_info("user_1", &[Coin::new(1000000, "uxyz")]),
        ExecuteMsg::Bond { receiver: None },
    )
    .unwrap();

    assert_eq!(res.messages.len(), 2);
    assert_eq!(
        res.messages[0],
        SubMsg::reply_on_success(
            Delegation::new("alice", 1000000, "uxyz")
                .to_cosmos_msg(env.contract.address.to_string())
                .unwrap(),
            REPLY_REGISTER_RECEIVED_COINS
        )
    );
    assert_eq!(
        res.messages[1],
        SubMsg {
            id: 0,
            msg: CosmosMsg::Wasm(WasmMsg::Execute {
                contract_addr: "steak_token".to_string(),
                msg: to_binary(&Cw20ExecuteMsg::Mint {
                    recipient: "user_1".to_string(),
                    amount: Uint128::new(1000000)
                })
                .unwrap(),
                funds: vec![]
            }),
            gas_limit: None,
            reply_on: ReplyOn::Never,
        }
    );

    // Bond when there are existing delegations, and Native Token:Steak exchange rate is >1
    // Previously user 1 delegated 1,000,000 native_token. We assume we have accumulated 2.5% yield at 1025000 staked
    deps.querier.set_staking_delegations(&[
        Delegation::new("alice", 341667, "uxyz"),
        Delegation::new("bob", 341667, "uxyz"),
        Delegation::new("charlie", 341666, "uxyz"),
    ]);
    deps.querier.set_cw20_total_supply("steak_token", 1000000);

    // Charlie has the smallest amount of delegation, so the full deposit goes to him
    let res = execute(
        deps.as_mut(),
        mock_env(),
        mock_info("user_2", &[Coin::new(12345, "uxyz")]),
        ExecuteMsg::Bond {
            receiver: Some("user_3".to_string()),
        },
    )
    .unwrap();

    assert_eq!(res.messages.len(), 2);
    assert_eq!(
        res.messages[0],
        SubMsg::reply_on_success(
            Delegation::new("charlie", 12345, "uxyz")
                .to_cosmos_msg(env.contract.address.to_string())
                .unwrap(),
            REPLY_REGISTER_RECEIVED_COINS
        )
    );
    assert_eq!(
        res.messages[1],
        SubMsg {
            id: 0,
            msg: CosmosMsg::Wasm(WasmMsg::Execute {
                contract_addr: "steak_token".to_string(),
                msg: to_binary(&Cw20ExecuteMsg::Mint {
                    recipient: "user_3".to_string(),
                    amount: Uint128::new(12043)
                })
                .unwrap(),
                funds: vec![]
            }),
            gas_limit: None,
            reply_on: ReplyOn::Never
        }
    );

    // Check the state after bonding
    deps.querier.set_staking_delegations(&[
        Delegation::new("alice", 341667, "uxyz"),
        Delegation::new("bob", 341667, "uxyz"),
        Delegation::new("charlie", 354011, "uxyz"),
    ]);
    deps.querier.set_cw20_total_supply("steak_token", 1012043);

    let res: StateResponse = query_helper(deps.as_ref(), QueryMsg::State {});
    assert_eq!(
        res,
        StateResponse {
            total_usteak: Uint128::new(1012043),
            total_native: Uint128::new(1037345),
            exchange_rate: Decimal::from_ratio(1037345u128, 1012043u128),
            unlocked_coins: vec![],
        }
    );
}

#[test]
fn harvesting() {
    let mut deps = setup_test();

    // Assume users have bonded a total of 1,000,000 native_token and minted the same amount of usteak
    deps.querier.set_staking_delegations(&[
        Delegation::new("alice", 341667, "uxyz"),
        Delegation::new("bob", 341667, "uxyz"),
        Delegation::new("charlie", 341666, "uxyz"),
    ]);
    deps.querier.set_cw20_total_supply("steak_token", 1000000);

    let harvest_env = mock_env();
    let res = execute(
        deps.as_mut(),
        harvest_env.clone(),
        mock_info(&harvest_env.contract.address.to_string(), &[]),
        ExecuteMsg::Harvest {},
    )
    .unwrap();

    assert_eq!(res.messages.len(), 4);
    assert_eq!(
        res.messages[0],
        SubMsg::reply_on_success(
            RewardWithdrawal {
                validator: "alice".to_string(),
            }
            .to_cosmos_msg(harvest_env.contract.address.to_string())
            .unwrap(),
            REPLY_REGISTER_RECEIVED_COINS,
        )
    );
    assert_eq!(
        res.messages[1],
        SubMsg::reply_on_success(
            RewardWithdrawal {
                validator: "bob".to_string(),
            }
            .to_cosmos_msg(harvest_env.contract.address.to_string())
            .unwrap(),
            REPLY_REGISTER_RECEIVED_COINS,
        )
    );
    assert_eq!(
        res.messages[2],
        SubMsg::reply_on_success(
            RewardWithdrawal {
                validator: "charlie".to_string(),
            }
            .to_cosmos_msg(harvest_env.contract.address.to_string())
            .unwrap(),
            REPLY_REGISTER_RECEIVED_COINS,
        )
    );
    assert_eq!(
        res.messages[3],
        SubMsg {
            id: 0,
            msg: CosmosMsg::Wasm(WasmMsg::Execute {
                contract_addr: MOCK_CONTRACT_ADDR.to_string(),
                msg: to_binary(&ExecuteMsg::Callback(CallbackMsg::Reinvest {})).unwrap(),
                funds: vec![]
            }),
            gas_limit: None,
            reply_on: ReplyOn::Never
        }
    );
}

#[test]
fn registering_unlocked_coins() {
    let mut deps = setup_test();
    let state = State::default();

    // After withdrawing staking rewards, we parse the `coin_received` event to find the received amounts
    let event = Event::new("coin_received")
        .add_attribute("receiver", MOCK_CONTRACT_ADDR.to_string())
        .add_attribute("amount", "123ukrw,234uxyz,345uusd,69420ibc/0471F1C4E7AFD3F07702BEF6DC365268D64570F7C1FDC98EA6098DD6DE59817B");

    reply(
        deps.as_mut(),
        mock_env(),
        Reply {
            id: 2,
            result: cosmwasm_std::SubMsgResult::Ok(SubMsgResponse {
                events: vec![event],
                data: None,
            }),
        },
    )
    .unwrap();

    // Unlocked coins in contract state should have been updated
    let unlocked_coins = state.unlocked_coins.load(deps.as_ref().storage).unwrap();
    assert_eq!(
        unlocked_coins,
        vec![
            Coin::new(123, "ukrw"),
            Coin::new(234, "uxyz"),
            Coin::new(345, "uusd"),
            Coin::new(
                69420,
                "ibc/0471F1C4E7AFD3F07702BEF6DC365268D64570F7C1FDC98EA6098DD6DE59817B"
            ),
        ]
    );
}

#[test]
fn reinvesting() {
    let mut deps = setup_test();
    let state = State::default();

    deps.querier.set_staking_delegations(&[
        Delegation::new("alice", 333334, "uxyz"),
        Delegation::new("bob", 333333, "uxyz"),
        Delegation::new("charlie", 333333, "uxyz"),
    ]);
    state
        .prev_denom
        .save(deps.as_mut().storage, &Uint128::from(0_u32))
        .unwrap();
    deps.querier
        .set_bank_balances(&[Coin::new(234u128, "uxyz")]);

    // After the swaps, `unlocked_coins` should contain only uxyz and unknown denoms
    state
        .unlocked_coins
        .save(
            deps.as_mut().storage,
            &vec![
                Coin::new(234, "uxyz"),
                Coin::new(
                    69420,
                    "ibc/0471F1C4E7AFD3F07702BEF6DC365268D64570F7C1FDC98EA6098DD6DE59817B",
                ),
            ],
        )
        .unwrap();

    let modifier = 1_000_000_000_000_000_000_u128;

    state
        .total_mining_power
        .save(deps.as_mut().storage, &Uint128::from(15_u128.mul(modifier)))
        .unwrap();

    state
        .validator_mining_powers
        .save(
            deps.as_mut().storage,
            "alice".to_string(),
            &5_u128.mul(modifier).into(),
        )
        .unwrap();
    state
        .validator_mining_powers
        .save(
            deps.as_mut().storage,
            "bob".to_string(),
            &5_u128.mul(modifier).into(),
        )
        .unwrap();
    state
        .validator_mining_powers
        .save(
            deps.as_mut().storage,
            "charlie".to_string(),
            &5_u128.mul(modifier).into(),
        )
        .unwrap();

    let env = mock_env();
    // Bob has the smallest amount of delegations, so all proceeds go to him
    let res = execute(
        deps.as_mut(),
        env.clone(),
        mock_info(MOCK_CONTRACT_ADDR, &[]),
        ExecuteMsg::Callback(CallbackMsg::Reinvest {}),
    )
    .unwrap();

    // decode first message as to MsgUndelegate
    let decoded_message =
        if let CosmosMsg::Stargate { type_url, value } = res.messages[0].msg.clone() {
            // assert_eq!(type_url, "/liquidstaking.staking.v1beta1.MsgDelegate");
            let msg_decoded: MsgDelegate = prost::Message::decode(value.as_slice()).unwrap();
            // assert_eq!(msg_decoded.validator_address, "bob");
            Some(msg_decoded)
        } else {
            None
        };
    // decode all messages to MsgUndelegate and transpose as result
    let decoded_messages = res
        .messages
        .iter()
        .map(|msg| {
            if let CosmosMsg::Stargate { type_url, value } = msg.msg.clone() {
                // assert_eq!(type_url, "/liquidstaking.staking.v1beta1.MsgDelegate");
                let msg_decoded: MsgDelegate = prost::Message::decode(value.as_slice()).unwrap();
                // assert_eq!(msg_decoded.validator_address, "bob");
                Some(msg_decoded)
            } else {
                None
            }
        })
        .filter(Option::is_some)
        .collect::<Option<Vec<MsgDelegate>>>()
        .unwrap();

    assert_eq!(res.messages.len(), 2);
    assert_eq!(
        res.messages[0],
        SubMsg {
            id: 0,
            msg: Delegation::new("bob", 234 - 23, "uxyz")
                .to_cosmos_msg(env.contract.address.to_string())
                .unwrap(),
            gas_limit: None,
            reply_on: ReplyOn::Never
        },
        "bob"
    );
    let send_msg = BankMsg::Send {
        to_address: "the_fee_man".into(),
        amount: vec![Coin::new(23u128, "uxyz")],
    };
    assert_eq!(
        res.messages[1],
        SubMsg {
            id: 0,
            msg: CosmosMsg::Bank(send_msg),
            gas_limit: None,
            reply_on: ReplyOn::Never
        },
        "fee"
    );

    // Storage should have been updated
    let unlocked_coins = state.unlocked_coins.load(deps.as_ref().storage).unwrap();
    assert_eq!(
        unlocked_coins,
        vec![Coin::new(
            69420,
            "ibc/0471F1C4E7AFD3F07702BEF6DC365268D64570F7C1FDC98EA6098DD6DE59817B"
        )],
        "unlocked_coins"
    );
}

#[test]
fn reinvesting_with_mining() {
    let mut deps = setup_test();
    let state = State::default();

    deps.querier.set_staking_delegations(&[
        Delegation::new("alice", 333334, "uxyz"),
        Delegation::new("bob", 333333, "uxyz"),
        Delegation::new("charlie", 333333, "uxyz"),
    ]);
    state
        .prev_denom
        .save(deps.as_mut().storage, &Uint128::from(0_u32))
        .unwrap();
    deps.querier
        .set_bank_balances(&[Coin::new(234u128, "uxyz")]);

    // After the swaps, `unlocked_coins` should contain only uxyz and unknown denoms
    state
        .unlocked_coins
        .save(
            deps.as_mut().storage,
            &vec![
                Coin::new(234, "uxyz"),
                Coin::new(
                    69420,
                    "ibc/0471F1C4E7AFD3F07702BEF6DC365268D64570F7C1FDC98EA6098DD6DE59817B",
                ),
            ],
        )
        .unwrap();

    let modifier = 1_000_000_000_000_000_000_u128;

    state
        .total_mining_power
        .save(deps.as_mut().storage, &Uint128::from(15_u128.mul(modifier)))
        .unwrap();

    state
        .validator_mining_powers
        .save(
            deps.as_mut().storage,
            "alice".to_string(),
            &4_u128.mul(modifier).into(),
        )
        .unwrap();
    state
        .validator_mining_powers
        .save(
            deps.as_mut().storage,
            "bob".to_string(),
            &4_u128.mul(modifier).into(),
        )
        .unwrap();
    state
        .validator_mining_powers
        .save(
            deps.as_mut().storage,
            "charlie".to_string(),
            &7_u128.mul(modifier).into(),
        )
        .unwrap();

    let env = mock_env();
    // Bob has the smallest amount of delegations, so all proceeds go to him
    let res = execute(
        deps.as_mut(),
        env.clone(),
        mock_info(MOCK_CONTRACT_ADDR, &[]),
        ExecuteMsg::Callback(CallbackMsg::Reinvest {}),
    )
    .unwrap();

    // decode first message as to MsgUndelegate
    let decoded_message =
        if let CosmosMsg::Stargate { type_url, value } = res.messages[0].msg.clone() {
            // assert_eq!(type_url, "/liquidstaking.staking.v1beta1.MsgDelegate");
            let msg_decoded: MsgDelegate = prost::Message::decode(value.as_slice()).unwrap();
            // assert_eq!(msg_decoded.validator_address, "bob");
            Some(msg_decoded)
        } else {
            None
        };
    // decode all messages to MsgUndelegate and transpose as result
    let decoded_messages = res
        .messages
        .iter()
        .map(|msg| {
            if let CosmosMsg::Stargate { type_url, value } = msg.msg.clone() {
                // assert_eq!(type_url, "/liquidstaking.staking.v1beta1.MsgDelegate");
                let msg_decoded: MsgDelegate = prost::Message::decode(value.as_slice()).unwrap();
                // assert_eq!(msg_decoded.validator_address, "bob");
                Some(msg_decoded)
            } else {
                None
            }
        })
        .filter(Option::is_some)
        .collect::<Option<Vec<MsgDelegate>>>()
        .unwrap();

    assert_eq!(res.messages.len(), 2);
    assert_eq!(
        res.messages[0],
        SubMsg {
            id: 0,
            msg: Delegation::new("charlie", 234 - 23, "uxyz")
                .to_cosmos_msg(env.contract.address.to_string())
                .unwrap(),
            gas_limit: None,
            reply_on: ReplyOn::Never
        },
        "charlie"
    );
    let send_msg = BankMsg::Send {
        to_address: "the_fee_man".into(),
        amount: vec![Coin::new(23u128, "uxyz")],
    };
    assert_eq!(
        res.messages[1],
        SubMsg {
            id: 0,
            msg: CosmosMsg::Bank(send_msg),
            gas_limit: None,
            reply_on: ReplyOn::Never
        },
        "fee"
    );

    // Storage should have been updated
    let unlocked_coins = state.unlocked_coins.load(deps.as_ref().storage).unwrap();
    assert_eq!(
        unlocked_coins,
        vec![Coin::new(
            69420,
            "ibc/0471F1C4E7AFD3F07702BEF6DC365268D64570F7C1FDC98EA6098DD6DE59817B"
        )],
        "unlocked_coins"
    );
}

#[test]
fn reinvesting_fee_split() {
    let mut deps = setup_test_fee_split();
    let state = State::default();
    let env = mock_env();
    deps.querier.set_staking_delegations(&[
        Delegation::new("alice", 333334, "uxyz"),
        Delegation::new("bob", 333333, "uxyz"),
        Delegation::new("charlie", 333333, "uxyz"),
    ]);
    state
        .prev_denom
        .save(deps.as_mut().storage, &Uint128::from(0_u32))
        .unwrap();
    deps.querier
        .set_bank_balances(&[Coin::new(234u128, "uxyz")]);

    // After the swaps, `unlocked_coins` should contain only uxyz and unknown denoms
    state
        .unlocked_coins
        .save(
            deps.as_mut().storage,
            &vec![
                Coin::new(234, "uxyz"),
                Coin::new(
                    69420,
                    "ibc/0471F1C4E7AFD3F07702BEF6DC365268D64570F7C1FDC98EA6098DD6DE59817B",
                ),
            ],
        )
        .unwrap();

    let modifier = 1_000_000_000_000_000_000_u128;

    state
        .total_mining_power
        .save(deps.as_mut().storage, &Uint128::from(15_u128.mul(modifier)))
        .unwrap();

    state
        .validator_mining_powers
        .save(
            deps.as_mut().storage,
            "alice".to_string(),
            &1_u128.mul(modifier).into(),
        )
        .unwrap();
    state
        .validator_mining_powers
        .save(
            deps.as_mut().storage,
            "bob".to_string(),
            &12_u128.mul(modifier).into(),
        )
        .unwrap();
    state
        .validator_mining_powers
        .save(
            deps.as_mut().storage,
            "charlie".to_string(),
            &2_u128.mul(modifier).into(),
        )
        .unwrap();

    // Bob has the smallest amount of delegations, so all proceeds go to him
    let res = execute(
        deps.as_mut(),
        env.clone(),
        mock_info(MOCK_CONTRACT_ADDR, &[]),
        ExecuteMsg::Callback(CallbackMsg::Reinvest {}),
    )
    .unwrap();

    assert_eq!(res.messages.len(), 2);
    assert_eq!(
        res.messages[0],
        SubMsg {
            id: 0,
            msg: Delegation::new("bob", 234 - 23, "uxyz")
                .to_cosmos_msg(env.contract.address.to_string())
                .unwrap(),
            gas_limit: None,
            reply_on: ReplyOn::Never
        }
    );
    let send_msg = pfc_fee_split::fee_split_msg::ExecuteMsg::Deposit { flush: false };

    assert_eq!(
        res.messages[1],
        SubMsg {
            id: 0,
            msg: send_msg
                .into_cosmos_msg("fee_split_contract", vec![Coin::new(23u128, "uxyz")])
                .unwrap(),
            gas_limit: None,
            reply_on: ReplyOn::Never
        }
    );

    // Storage should have been updated
    let unlocked_coins = state.unlocked_coins.load(deps.as_ref().storage).unwrap();
    assert_eq!(
        unlocked_coins,
        vec![Coin::new(
            69420,
            "ibc/0471F1C4E7AFD3F07702BEF6DC365268D64570F7C1FDC98EA6098DD6DE59817B"
        )],
    );
}

#[test]
fn queuing_unbond() {
    let mut deps = setup_test();
    let state = State::default();

    // Only Steak token is accepted for unbonding requests
    let err = execute(
        deps.as_mut(),
        mock_env(),
        mock_info("random_token", &[]),
        ExecuteMsg::Receive(cw20::Cw20ReceiveMsg {
            sender: "hacker".to_string(),
            amount: Uint128::new(69420),
            msg: to_binary(&ReceiveMsg::QueueUnbond { receiver: None }).unwrap(),
        }),
    )
    .unwrap_err();

    assert_eq!(
        err,
        StdError::generic_err("expecting Steak token, received random_token")
    );

    // User 1 creates an unbonding request before `est_unbond_start_time` is reached. The unbond
    // request is saved, but not the pending batch is not submitted for unbonding
    let res = execute(
        deps.as_mut(),
        mock_env_at_timestamp(12345), // est_unbond_start_time = 269200
        mock_info("steak_token", &[]),
        ExecuteMsg::Receive(cw20::Cw20ReceiveMsg {
            sender: "user_1".to_string(),
            amount: Uint128::new(23456),
            msg: to_binary(&ReceiveMsg::QueueUnbond { receiver: None }).unwrap(),
        }),
    )
    .unwrap();

    assert_eq!(res.messages.len(), 0);

    // User 2 creates an unbonding request after `est_unbond_start_time` is reached. The unbond
    // request is saved, and the pending is automatically submitted for unbonding
    let res = execute(
        deps.as_mut(),
        mock_env_at_timestamp(269201), // est_unbond_start_time = 269200
        mock_info("steak_token", &[]),
        ExecuteMsg::Receive(cw20::Cw20ReceiveMsg {
            sender: "user_2".to_string(),
            amount: Uint128::new(69420),
            msg: to_binary(&ReceiveMsg::QueueUnbond {
                receiver: Some("user_3".to_string()),
            })
            .unwrap(),
        }),
    )
    .unwrap();

    assert_eq!(res.messages.len(), 1);
    assert_eq!(
        res.messages[0],
        SubMsg {
            id: 0,
            msg: CosmosMsg::Wasm(WasmMsg::Execute {
                contract_addr: MOCK_CONTRACT_ADDR.to_string(),
                msg: to_binary(&ExecuteMsg::SubmitBatch {}).unwrap(),
                funds: vec![]
            }),
            gas_limit: None,
            reply_on: ReplyOn::Never
        }
    );

    // The users' unbonding requests should have been saved
    let ubr1 = state
        .unbond_requests
        .load(deps.as_ref().storage, (1u64, &Addr::unchecked("user_1")))
        .unwrap();
    let ubr2 = state
        .unbond_requests
        .load(deps.as_ref().storage, (1u64, &Addr::unchecked("user_3")))
        .unwrap();

    assert_eq!(
        ubr1,
        UnbondRequest {
            id: 1,
            user: Addr::unchecked("user_1"),
            shares: Uint128::new(23456)
        }
    );
    assert_eq!(
        ubr2,
        UnbondRequest {
            id: 1,
            user: Addr::unchecked("user_3"),
            shares: Uint128::new(69420)
        }
    );

    // Pending batch should have been updated
    let pending_batch = state.pending_batch.load(deps.as_ref().storage).unwrap();
    assert_eq!(
        pending_batch,
        PendingBatch {
            id: 1,
            usteak_to_burn: Uint128::new(92876), // 23,456 + 69,420
            est_unbond_start_time: 269200
        }
    );
}

#[test]
fn submitting_batch() {
    let mut deps = setup_test();
    let state = State::default();

    // native_token bonded: 1,037,345
    // usteak supply: 1,012,043
    // native_token per ustake: 1.025
    deps.querier.set_staking_delegations(&[
        Delegation::new("alice", 345782, "uxyz"),
        Delegation::new("bob", 345782, "uxyz"),
        Delegation::new("charlie", 345781, "uxyz"),
    ]);
    deps.querier.set_cw20_total_supply("steak_token", 1012043);

    // We continue from the contract state at the end of the last test
    let unbond_requests = vec![
        UnbondRequest {
            id: 1,
            user: Addr::unchecked("user_1"),
            shares: Uint128::new(23456),
        },
        UnbondRequest {
            id: 1,
            user: Addr::unchecked("user_3"),
            shares: Uint128::new(69420),
        },
    ];

    for unbond_request in &unbond_requests {
        state
            .unbond_requests
            .save(
                deps.as_mut().storage,
                (
                    unbond_request.id,
                    &Addr::unchecked(unbond_request.user.clone()),
                ),
                unbond_request,
            )
            .unwrap();
    }

    state
        .pending_batch
        .save(
            deps.as_mut().storage,
            &PendingBatch {
                id: 1,
                usteak_to_burn: Uint128::new(92876), // 23,456 + 69,420
                est_unbond_start_time: 269200,
            },
        )
        .unwrap();

    // Anyone can invoke `submit_batch`. Here we continue from the previous test and assume it is
    // invoked automatically as user 2 submits the unbonding request
    //
    // usteak to burn: 23,456 + 69,420 = 92,876
    // native_token to unbond: 1,037,345 * 92,876 / 1,012,043 = 95,197
    //
    // Target: (1,037,345 - 95,197) / 3 = 314,049
    // Remainer: 1
    // Alice:   345,782 - (314,049 + 1) = 31,732
    // Bob:     345,782 - (314,049 + 0) = 31,733
    // Charlie: 345,781 - (314,049 + 0) = 31,732
    let env_at_ts = mock_env_at_timestamp(269201);
    let res = execute(
        deps.as_mut(),
        env_at_ts.clone(),
        mock_info(MOCK_CONTRACT_ADDR, &[]),
        ExecuteMsg::SubmitBatch {},
    )
    .unwrap();

    assert_eq!(res.messages.len(), 4);
    assert_eq!(
        res.messages[0],
        SubMsg::reply_on_success(
            Undelegation::new("alice", 31732, "uxyz")
                .to_cosmos_msg(env_at_ts.contract.address.to_string())
                .unwrap(),
            REPLY_REGISTER_RECEIVED_COINS
        )
    );
    assert_eq!(
        res.messages[1],
        SubMsg::reply_on_success(
            Undelegation::new("bob", 31733, "uxyz")
                .to_cosmos_msg(env_at_ts.contract.address.to_string())
                .unwrap(),
            REPLY_REGISTER_RECEIVED_COINS
        )
    );
    assert_eq!(
        res.messages[2],
        SubMsg::reply_on_success(
            Undelegation::new("charlie", 31732, "uxyz")
                .to_cosmos_msg(env_at_ts.contract.address.to_string())
                .unwrap(),
            REPLY_REGISTER_RECEIVED_COINS
        )
    );
    assert_eq!(
        res.messages[3],
        SubMsg {
            id: 0,
            msg: CosmosMsg::Wasm(WasmMsg::Execute {
                contract_addr: "steak_token".to_string(),
                msg: to_binary(&Cw20ExecuteMsg::Burn {
                    amount: Uint128::new(92876)
                })
                .unwrap(),
                funds: vec![]
            }),
            gas_limit: None,
            reply_on: ReplyOn::Never
        }
    );

    // A new pending batch should have been created
    let pending_batch = state.pending_batch.load(deps.as_ref().storage).unwrap();
    assert_eq!(
        pending_batch,
        PendingBatch {
            id: 2,
            usteak_to_burn: Uint128::zero(),
            est_unbond_start_time: 528401 // 269,201 + 259,200
        }
    );

    // Previous batch should have been updated
    let previous_batch = state
        .previous_batches
        .load(deps.as_ref().storage, 1u64)
        .unwrap();
    assert_eq!(
        previous_batch,
        Batch {
            id: 1,
            reconciled: false,
            total_shares: Uint128::new(92876),
            amount_unclaimed: Uint128::new(95197),
            est_unbond_end_time: 2083601 // 269,201 + 1,814,400
        }
    );
}

#[test]
fn reconciling() {
    let mut deps = setup_test();
    let state = State::default();

    let previous_batches = vec![
        Batch {
            id: 1,
            reconciled: true,
            total_shares: Uint128::new(92876),
            amount_unclaimed: Uint128::new(95197), // 1.025 Native Token per Steak
            est_unbond_end_time: 10000,
        },
        Batch {
            id: 2,
            reconciled: false,
            total_shares: Uint128::new(1345),
            amount_unclaimed: Uint128::new(1385), // 1.030 Native Token per Steak
            est_unbond_end_time: 20000,
        },
        Batch {
            id: 3,
            reconciled: false,
            total_shares: Uint128::new(1456),
            amount_unclaimed: Uint128::new(1506), // 1.035 Native Token per Steak
            est_unbond_end_time: 30000,
        },
        Batch {
            id: 4,
            reconciled: false,
            total_shares: Uint128::new(1567),
            amount_unclaimed: Uint128::new(1629), // 1.040 Native Token per Steak
            est_unbond_end_time: 40000,           // not yet finished unbonding, ignored
        },
    ];

    for previous_batch in &previous_batches {
        state
            .previous_batches
            .save(deps.as_mut().storage, previous_batch.id, previous_batch)
            .unwrap();
    }

    state
        .unlocked_coins
        .save(
            deps.as_mut().storage,
            &vec![
                Coin::new(10000, "uxyz"),
                Coin::new(234, "ukrw"),
                Coin::new(345, "uusd"),
                Coin::new(
                    69420,
                    "ibc/0471F1C4E7AFD3F07702BEF6DC365268D64570F7C1FDC98EA6098DD6DE59817B",
                ),
            ],
        )
        .unwrap();

    deps.querier.set_bank_balances(&[
        Coin::new(12345, "uxyz"),
        Coin::new(234, "ukrw"),
        Coin::new(345, "uusd"),
        Coin::new(
            69420,
            "ibc/0471F1C4E7AFD3F07702BEF6DC365268D64570F7C1FDC98EA6098DD6DE59817B",
        ),
    ]);

    execute(
        deps.as_mut(),
        mock_env_at_timestamp(35000),
        mock_info("worker", &[]),
        ExecuteMsg::Reconcile {},
    )
    .unwrap();

    // Expected received: batch 2 + batch 3 = 1385 + 1506 = 2891
    // Expected unlocked: 10000
    // Expected: 12891
    // Actual: 12345
    // Shortfall: 12891 - 12345 = 456
    //
    // native_token per batch: 546 / 2 = 273
    // remainder: 0
    // batch 2: 1385 - 273 = 1112
    // batch 3: 1506 - 273 = 1233
    let batch = state
        .previous_batches
        .load(deps.as_ref().storage, 2u64)
        .unwrap();
    assert_eq!(
        batch,
        Batch {
            id: 2,
            reconciled: true,
            total_shares: Uint128::new(1345),
            amount_unclaimed: Uint128::new(1112), // 1385 - 273
            est_unbond_end_time: 20000,
        }
    );

    let batch = state
        .previous_batches
        .load(deps.as_ref().storage, 3u64)
        .unwrap();
    assert_eq!(
        batch,
        Batch {
            id: 3,
            reconciled: true,
            total_shares: Uint128::new(1456),
            amount_unclaimed: Uint128::new(1233), // 1506 - 273
            est_unbond_end_time: 30000,
        }
    );

    // Batches 1 and 4 should not have changed
    let batch = state
        .previous_batches
        .load(deps.as_ref().storage, 1u64)
        .unwrap();
    assert_eq!(batch, previous_batches[0]);

    let batch = state
        .previous_batches
        .load(deps.as_ref().storage, 4u64)
        .unwrap();
    assert_eq!(batch, previous_batches[3]);
}

#[test]
fn withdrawing_unbonded() {
    let mut deps = setup_test();
    let state = State::default();

    // We simulate a most general case:
    // - batches 1 and 2 have finished unbonding
    // - batch 3 have been submitted for unbonding but have not finished
    // - batch 4 is still pending
    let unbond_requests = vec![
        UnbondRequest {
            id: 1,
            user: Addr::unchecked("user_1"),
            shares: Uint128::new(23456),
        },
        UnbondRequest {
            id: 1,
            user: Addr::unchecked("user_3"),
            shares: Uint128::new(69420),
        },
        UnbondRequest {
            id: 2,
            user: Addr::unchecked("user_1"),
            shares: Uint128::new(34567),
        },
        UnbondRequest {
            id: 3,
            user: Addr::unchecked("user_1"),
            shares: Uint128::new(45678),
        },
        UnbondRequest {
            id: 4,
            user: Addr::unchecked("user_1"),
            shares: Uint128::new(56789),
        },
    ];

    for unbond_request in &unbond_requests {
        state
            .unbond_requests
            .save(
                deps.as_mut().storage,
                (
                    unbond_request.id,
                    &Addr::unchecked(unbond_request.user.clone()),
                ),
                unbond_request,
            )
            .unwrap();
    }

    let previous_batches = vec![
        Batch {
            id: 1,
            reconciled: true,
            total_shares: Uint128::new(92876),
            amount_unclaimed: Uint128::new(95197), // 1.025 Native Token per Steak
            est_unbond_end_time: 10000,
        },
        Batch {
            id: 2,
            reconciled: true,
            total_shares: Uint128::new(34567),
            amount_unclaimed: Uint128::new(35604), // 1.030 Native Token per Steak
            est_unbond_end_time: 20000,
        },
        Batch {
            id: 3,
            reconciled: false, // finished unbonding, but not reconciled; ignored
            total_shares: Uint128::new(45678),
            amount_unclaimed: Uint128::new(47276), // 1.035 Native Token per Steak
            est_unbond_end_time: 20000,
        },
        Batch {
            id: 4,
            reconciled: true,
            total_shares: Uint128::new(56789),
            amount_unclaimed: Uint128::new(59060), // 1.040 Native Token per Steak
            est_unbond_end_time: 30000, // reconciled, but not yet finished unbonding; ignored
        },
    ];

    for previous_batch in &previous_batches {
        state
            .previous_batches
            .save(deps.as_mut().storage, previous_batch.id, previous_batch)
            .unwrap();
    }

    state
        .pending_batch
        .save(
            deps.as_mut().storage,
            &PendingBatch {
                id: 4,
                usteak_to_burn: Uint128::new(56789),
                est_unbond_start_time: 100000,
            },
        )
        .unwrap();

    // Attempt to withdraw before any batch has completed unbonding. Should error
    let err = execute(
        deps.as_mut(),
        mock_env_at_timestamp(5000),
        mock_info("user_1", &[]),
        ExecuteMsg::WithdrawUnbonded { receiver: None },
    )
    .unwrap_err();

    assert_eq!(err, StdError::generic_err("withdrawable amount is zero"));

    // Attempt to withdraw once batches 1 and 2 have finished unbonding, but 3 has not yet
    //
    // Withdrawable from batch 1: 95,197 * 23,456 / 92,876 = 24,042
    // Withdrawable from batch 2: 35,604
    // Total withdrawable: 24,042 + 35,604 = 59,646
    //
    // Batch 1 should be updated:
    // Total shares: 92,876 - 23,456 = 69,420
    // Unclaimed native_token: 95,197 - 24,042 = 71,155
    //
    // Batch 2 is completely withdrawn, should be purged from storage
    let res = execute(
        deps.as_mut(),
        mock_env_at_timestamp(25000),
        mock_info("user_1", &[]),
        ExecuteMsg::WithdrawUnbonded { receiver: None },
    )
    .unwrap();

    assert_eq!(res.messages.len(), 1);
    assert_eq!(
        res.messages[0],
        SubMsg {
            id: 0,
            msg: CosmosMsg::Bank(BankMsg::Send {
                to_address: "user_1".to_string(),
                amount: vec![Coin::new(59646, "uxyz")]
            }),
            gas_limit: None,
            reply_on: ReplyOn::Never
        }
    );

    // Previous batches should have been updated
    let batch = state
        .previous_batches
        .load(deps.as_ref().storage, 1u64)
        .unwrap();
    assert_eq!(
        batch,
        Batch {
            id: 1,
            reconciled: true,
            total_shares: Uint128::new(69420),
            amount_unclaimed: Uint128::new(71155),
            est_unbond_end_time: 10000,
        }
    );

    let err = state
        .previous_batches
        .load(deps.as_ref().storage, 2u64)
        .unwrap_err();
    assert_eq!(err, StdError::not_found("pfc_steak::hub::Batch"));

    // User 1's unbond requests in batches 1 and 2 should have been deleted
    let err1 = state
        .unbond_requests
        .load(deps.as_ref().storage, (1u64, &Addr::unchecked("user_1")))
        .unwrap_err();
    let err2 = state
        .unbond_requests
        .load(deps.as_ref().storage, (1u64, &Addr::unchecked("user_1")))
        .unwrap_err();

    assert_eq!(err1, StdError::not_found("pfc_steak::hub::UnbondRequest"));
    assert_eq!(err2, StdError::not_found("pfc_steak::hub::UnbondRequest"));
    // User 3 attempt to withdraw; also specifying a receiver
    let res = execute(
        deps.as_mut(),
        mock_env_at_timestamp(25000),
        mock_info("user_3", &[]),
        ExecuteMsg::WithdrawUnbonded {
            receiver: Some("user_2".to_string()),
        },
    )
    .unwrap();

    assert_eq!(res.messages.len(), 1);
    assert_eq!(
        res.messages[0],
        SubMsg {
            id: 0,
            msg: CosmosMsg::Bank(BankMsg::Send {
                to_address: "user_2".to_string(),
                amount: vec![Coin::new(71155, "uxyz")]
            }),
            gas_limit: None,
            reply_on: ReplyOn::Never
        }
    );

    // Batch 1 and user 2's unbonding request should have been purged from storage
    let err = state
        .previous_batches
        .load(deps.as_ref().storage, 1u64)
        .unwrap_err();
    assert_eq!(err, StdError::not_found("pfc_steak::hub::Batch"));

    let err = state
        .unbond_requests
        .load(deps.as_ref().storage, (1u64, &Addr::unchecked("user_3")))
        .unwrap_err();

    assert_eq!(err, StdError::not_found("pfc_steak::hub::UnbondRequest"));
}

#[test]
fn adding_validator() {
    let mut deps = setup_test();
    let state = State::default();

    let err = execute(
        deps.as_mut(),
        mock_env(),
        mock_info("jake", &[]),
        ExecuteMsg::AddValidator {
            validator: "dave".to_string(),
        },
    )
    .unwrap_err();

    assert_eq!(
        err,
        StdError::generic_err("unauthorized: sender is not owner")
    );

    let err = execute(
        deps.as_mut(),
        mock_env(),
        mock_info("larry", &[]),
        ExecuteMsg::AddValidator {
            validator: "alice".to_string(),
        },
    )
    .unwrap_err();

    assert_eq!(
        err,
        StdError::generic_err("validator is already whitelisted")
    );

    let res = execute(
        deps.as_mut(),
        mock_env(),
        mock_info("larry", &[]),
        ExecuteMsg::AddValidator {
            validator: "dave".to_string(),
        },
    )
    .unwrap();

    assert_eq!(res.messages.len(), 0);

    let validators = state.validators.load(deps.as_ref().storage).unwrap();
    assert_eq!(
        validators,
        vec![
            String::from("alice"),
            String::from("bob"),
            String::from("charlie"),
            String::from("dave")
        ],
    );
}

#[test]
fn removing_validator() {
    let mut deps = setup_test();
    let state = State::default();

    deps.querier.set_staking_delegations(&[
        Delegation::new("alice", 341667, "uxyz"),
        Delegation::new("bob", 341667, "uxyz"),
        Delegation::new("charlie", 341666, "uxyz"),
    ]);

    let err = execute(
        deps.as_mut(),
        mock_env(),
        mock_info("jake", &[]),
        ExecuteMsg::RemoveValidator {
            validator: "charlie".to_string(),
        },
    )
    .unwrap_err();

    assert_eq!(
        err,
        StdError::generic_err("unauthorized: sender is not owner")
    );

    let err = execute(
        deps.as_mut(),
        mock_env(),
        mock_info("larry", &[]),
        ExecuteMsg::RemoveValidator {
            validator: "dave".to_string(),
        },
    )
    .unwrap_err();

    assert_eq!(
        err,
        StdError::generic_err("validator is not already whitelisted")
    );

    // Target: (341667 + 341667 + 341666) / 2 = 512500
    // Remainder: 0
    // Alice:   512500 + 0 - 341667 = 170833
    // Bob:     512500 + 0 - 341667 = 170833
    let env = mock_env();
    let res = execute(
        deps.as_mut(),
        env.clone(),
        mock_info("larry", &[]),
        ExecuteMsg::RemoveValidator {
            validator: "charlie".to_string(),
        },
    )
    .unwrap();

    assert_eq!(res.messages.len(), 2);
    assert_eq!(
        res.messages[0],
        SubMsg::reply_on_success(
            Redelegation::new("charlie", "alice", 170833, "uxyz")
                .to_cosmos_msg(env.contract.address.to_string())
                .unwrap(),
            REPLY_REGISTER_RECEIVED_COINS
        ),
    );
    assert_eq!(
        res.messages[1],
        SubMsg::reply_on_success(
            Redelegation::new("charlie", "bob", 170833, "uxyz")
                .to_cosmos_msg(env.contract.address.to_string())
                .unwrap(),
            REPLY_REGISTER_RECEIVED_COINS
        ),
    );

    let validators = state.validators.load(deps.as_ref().storage).unwrap();
    assert_eq!(validators, vec![String::from("alice"), String::from("bob")],);
}

#[test]
fn transferring_ownership() {
    let mut deps = setup_test();
    let state = State::default();

    let err = execute(
        deps.as_mut(),
        mock_env(),
        mock_info("jake", &[]),
        ExecuteMsg::TransferOwnership {
            new_owner: "jake".to_string(),
        },
    )
    .unwrap_err();

    assert_eq!(
        err,
        StdError::generic_err("unauthorized: sender is not owner")
    );

    let res = execute(
        deps.as_mut(),
        mock_env(),
        mock_info("larry", &[]),
        ExecuteMsg::TransferOwnership {
            new_owner: "jake".to_string(),
        },
    )
    .unwrap();

    assert_eq!(res.messages.len(), 0);

    let owner = state.owner.load(deps.as_ref().storage).unwrap();
    assert_eq!(owner, Addr::unchecked("larry"));

    let err = execute(
        deps.as_mut(),
        mock_env(),
        mock_info("pumpkin", &[]),
        ExecuteMsg::AcceptOwnership {},
    )
    .unwrap_err();

    assert_eq!(
        err,
        StdError::generic_err("unauthorized: sender is not new owner")
    );

    let res = execute(
        deps.as_mut(),
        mock_env(),
        mock_info("jake", &[]),
        ExecuteMsg::AcceptOwnership {},
    )
    .unwrap();

    assert_eq!(res.messages.len(), 0);

    let owner = state.owner.load(deps.as_ref().storage).unwrap();
    assert_eq!(owner, Addr::unchecked("jake"));
}

#[test]
fn splitting_fees() {
    let mut deps = setup_test();

    let err = execute(
        deps.as_mut(),
        mock_env(),
        mock_info("jake", &[]),
        ExecuteMsg::TransferFeeAccount {
            fee_account_type: "Wallet".to_string(),
            new_fee_account: "charlie".to_string(),
        },
    )
    .unwrap_err();

    assert_eq!(
        err,
        StdError::generic_err("unauthorized: sender is not owner")
    );

    let err = execute(
        deps.as_mut(),
        mock_env(),
        mock_info("larry", &[]),
        ExecuteMsg::TransferFeeAccount {
            fee_account_type: "xxxx".to_string(),
            new_fee_account: "charlie".to_string(),
        },
    )
    .unwrap_err();

    assert_eq!(
        err,
        StdError::generic_err("Invalid Fee type: Wallet or FeeSplit only")
    );

    execute(
        deps.as_mut(),
        mock_env(),
        mock_info("larry", &[]),
        ExecuteMsg::TransferFeeAccount {
            fee_account_type: "Wallet".to_string(),
            new_fee_account: "charlie".to_string(),
        },
    )
    .unwrap();
    let res: ConfigResponse = query_helper(deps.as_ref(), QueryMsg::Config {});
    assert_eq!(
        res,
        ConfigResponse {
            owner: "larry".to_string(),
            new_owner: None,
            steak_token: "steak_token".to_string(),
            epoch_period: 259200,
            unbond_period: 1814400,
            denom: "uxyz".to_string(),
            fee_type: "Wallet".to_string(),
            fee_account: "charlie".to_string(),
            fee_rate: Decimal::from_ratio(10_u128, 100_u128),
            max_fee_rate: Decimal::from_ratio(20_u128, 100_u128),
            validators: vec![
                "alice".to_string(),
                "bob".to_string(),
                "charlie".to_string()
            ]
        }
    );

    execute(
        deps.as_mut(),
        mock_env(),
        mock_info("larry", &[]),
        ExecuteMsg::TransferFeeAccount {
            fee_account_type: "FeeSplit".to_string(),
            new_fee_account: "contract".to_string(),
        },
    )
    .unwrap();
    let res: ConfigResponse = query_helper(deps.as_ref(), QueryMsg::Config {});
    assert_eq!(
        res,
        ConfigResponse {
            owner: "larry".to_string(),
            new_owner: None,
            steak_token: "steak_token".to_string(),
            epoch_period: 259200,
            unbond_period: 1814400,
            denom: "uxyz".to_string(),
            fee_type: "FeeSplit".to_string(),
            fee_account: "contract".to_string(),
            fee_rate: Decimal::from_ratio(10_u128, 100_u128),
            max_fee_rate: Decimal::from_ratio(20_u128, 100_u128),
            validators: vec![
                "alice".to_string(),
                "bob".to_string(),
                "charlie".to_string()
            ]
        }
    );
}

// Test for ExecuteMsg::SubmitProof { nonce: Uint64, validator: String  }
#[test]
fn submit_proof() {
    let mut deps = setup_test();
    let state = State::default();
    let miner_entropy =
        "df5c2d1c1e799c13e81ef0d24acdb338e9da760af9afcd1bfbde40d61fed8996".to_string();
    let miner_address = "joe1gh9nds8amsy33ewpt97gj4n99436hftz2zl79q".to_string();
    let nonce = Uint64::from(121063160u64);
    deps.querier.set_staking_delegations(&[
        Delegation::new("alice", 341667, "uxyz"),
        Delegation::new("bob", 341667, "uxyz"),
        Delegation::new("charlie", 341666, "uxyz"),
    ]);
    state
        .miner_entropy
        .save(deps.as_mut().storage, &miner_entropy)
        .unwrap();
    state
        .miner_difficulty
        .save(deps.as_mut().storage, &Uint64::new(5))
        .unwrap();
    let err = execute(
        deps.as_mut(),
        mock_env(),
        mock_info(&miner_address.to_string(), &[]),
        ExecuteMsg::SubmitProof {
            nonce,
            validator: "alice".to_string(),
        },
    )
    .unwrap();
}

//--------------------------------------------------------------------------------------------------
// Queries
//--------------------------------------------------------------------------------------------------

#[test]
fn querying_previous_batches() {
    let mut deps = mock_dependencies();

    let batches = vec![
        Batch {
            id: 1,
            reconciled: false,
            total_shares: Uint128::new(123),
            amount_unclaimed: Uint128::new(678),
            est_unbond_end_time: 10000,
        },
        Batch {
            id: 2,
            reconciled: true,
            total_shares: Uint128::new(234),
            amount_unclaimed: Uint128::new(789),
            est_unbond_end_time: 15000,
        },
        Batch {
            id: 3,
            reconciled: false,
            total_shares: Uint128::new(345),
            amount_unclaimed: Uint128::new(890),
            est_unbond_end_time: 20000,
        },
        Batch {
            id: 4,
            reconciled: true,
            total_shares: Uint128::new(456),
            amount_unclaimed: Uint128::new(999),
            est_unbond_end_time: 25000,
        },
    ];

    let state = State::default();
    for batch in &batches {
        state
            .previous_batches
            .save(deps.as_mut().storage, batch.id, batch)
            .unwrap();
    }

    // Querying a single batch
    let res: Batch = query_helper(deps.as_ref(), QueryMsg::PreviousBatch(1));
    assert_eq!(res, batches[0].clone());

    let res: Batch = query_helper(deps.as_ref(), QueryMsg::PreviousBatch(2));
    assert_eq!(res, batches[1].clone());

    // Query multiple batches
    let res: Vec<Batch> = query_helper(
        deps.as_ref(),
        QueryMsg::PreviousBatches {
            start_after: None,
            limit: None,
        },
    );
    assert_eq!(res, batches);

    let res: Vec<Batch> = query_helper(
        deps.as_ref(),
        QueryMsg::PreviousBatches {
            start_after: Some(1),
            limit: None,
        },
    );
    assert_eq!(
        res,
        vec![batches[1].clone(), batches[2].clone(), batches[3].clone()]
    );

    let res: Vec<Batch> = query_helper(
        deps.as_ref(),
        QueryMsg::PreviousBatches {
            start_after: Some(4),
            limit: None,
        },
    );
    assert_eq!(res, vec![]);

    // Query multiple batches, indexed by whether it has been reconciled
    let res = state
        .previous_batches
        .idx
        .reconciled
        .prefix(true.into())
        .range(deps.as_ref().storage, None, None, Order::Ascending)
        .map(|item| {
            let (_, v) = item.unwrap();
            v
        })
        .collect::<Vec<_>>();

    assert_eq!(res, vec![batches[1].clone(), batches[3].clone()]);

    let res = state
        .previous_batches
        .idx
        .reconciled
        .prefix(false.into())
        .range(deps.as_ref().storage, None, None, Order::Ascending)
        .map(|item| {
            let (_, v) = item.unwrap();
            v
        })
        .collect::<Vec<_>>();

    assert_eq!(res, vec![batches[0].clone(), batches[2].clone()]);
}

#[test]
fn querying_unbond_requests() {
    let mut deps = mock_dependencies();
    let state = State::default();

    let unbond_requests = vec![
        UnbondRequest {
            id: 1,
            user: Addr::unchecked("alice"),
            shares: Uint128::new(123),
        },
        UnbondRequest {
            id: 1,
            user: Addr::unchecked("bob"),
            shares: Uint128::new(234),
        },
        UnbondRequest {
            id: 1,
            user: Addr::unchecked("charlie"),
            shares: Uint128::new(345),
        },
        UnbondRequest {
            id: 2,
            user: Addr::unchecked("alice"),
            shares: Uint128::new(456),
        },
    ];

    for unbond_request in &unbond_requests {
        state
            .unbond_requests
            .save(
                deps.as_mut().storage,
                (
                    unbond_request.id,
                    &Addr::unchecked(unbond_request.user.clone()),
                ),
                unbond_request,
            )
            .unwrap();
    }

    let res: Vec<UnbondRequestsByBatchResponseItem> = query_helper(
        deps.as_ref(),
        QueryMsg::UnbondRequestsByBatch {
            id: 1,
            start_after: None,
            limit: None,
        },
    );
    assert_eq!(
        res,
        vec![
            unbond_requests[0].clone().into(),
            unbond_requests[1].clone().into(),
            unbond_requests[2].clone().into(),
        ]
    );

    let res: Vec<UnbondRequestsByBatchResponseItem> = query_helper(
        deps.as_ref(),
        QueryMsg::UnbondRequestsByBatch {
            id: 2,
            start_after: None,
            limit: None,
        },
    );
    assert_eq!(res, vec![unbond_requests[3].clone().into()]);

    let res: Vec<UnbondRequestsByUserResponseItem> = query_helper(
        deps.as_ref(),
        QueryMsg::UnbondRequestsByUser {
            user: "alice".to_string(),
            start_after: None,
            limit: None,
        },
    );
    assert_eq!(
        res,
        vec![
            unbond_requests[0].clone().into(),
            unbond_requests[3].clone().into()
        ]
    );

    let res: Vec<UnbondRequestsByUserResponseItem> = query_helper(
        deps.as_ref(),
        QueryMsg::UnbondRequestsByUser {
            user: "alice".to_string(),
            start_after: Some(2),
            limit: None,
        },
    );
    assert_eq!(res, vec![unbond_requests[3].clone().into()]);
}

//--------------------------------------------------------------------------------------------------
// Delegations
//--------------------------------------------------------------------------------------------------

#[test]
fn computing_undelegations() {
    let current_delegations = vec![
        Delegation::new("alice", 400, "uxyz"),
        Delegation::new("bob", 300, "uxyz"),
        Delegation::new("charlie", 200, "uxyz"),
    ];

    // Target: (400 + 300 + 200 - 451) / 3 = 149
    // Remainder: 2
    // Alice:   400 - (149 + 1) = 250
    // Bob:     300 - (149 + 1) = 150
    // Charlie: 200 - (149 + 0) = 51
    let new_undelegations = compute_undelegations(Uint128::new(451), &current_delegations, "uxyz");
    let expected = vec![
        Undelegation::new("alice", 250, "uxyz"),
        Undelegation::new("bob", 150, "uxyz"),
        Undelegation::new("charlie", 51, "uxyz"),
    ];
    assert_eq!(new_undelegations, expected);
}

#[test]
fn computing_redelegations_for_removal() {
    let current_delegations = vec![
        Delegation::new("alice", 13000, "uxyz"),
        Delegation::new("bob", 12000, "uxyz"),
        Delegation::new("charlie", 11000, "uxyz"),
        Delegation::new("dave", 10000, "uxyz"),
    ];

    // Suppose Dave will be removed
    // native_token_per_validator = (13000 + 12000 + 11000 + 10000) / 3 = 15333
    // remainder = 1
    // to Alice:   15333 + 1 - 13000 = 2334
    // to Bob:     15333 + 0 - 12000 = 3333
    // to Charlie: 15333 + 0 - 11000 = 4333
    let expected = vec![
        Redelegation::new("dave", "alice", 2334, "uxyz"),
        Redelegation::new("dave", "bob", 3333, "uxyz"),
        Redelegation::new("dave", "charlie", 4333, "uxyz"),
    ];

    assert_eq!(
        compute_redelegations_for_removal(
            &current_delegations[3],
            &current_delegations[..3],
            "uxyz"
        ),
        expected,
    );
}

#[test]
fn computing_redelegations_for_rebalancing() {
    let current_delegations = vec![
        Delegation::new("alice", 69420, "uxyz"),
        Delegation::new("bob", 1234, "uxyz"),
        Delegation::new("charlie", 88888, "uxyz"),
        Delegation::new("dave", 40471, "uxyz"),
        Delegation::new("evan", 2345, "uxyz"),
    ];
    let active_validators: Vec<String> = vec![
        "alice".to_string(),
        "bob".to_string(),
        "charlie".to_string(),
        "dave".to_string(),
        "evan".to_string(),
    ];
    // native_token_per_validator = (69420 + 88888 + 1234 + 40471 + 2345) / 4 = 40471
    // remainer = 3
    // src_delegations:
    //  - alice:   69420 - (40471 + 1) = 28948
    //  - charlie: 88888 - (40471 + 1) = 48416
    // dst_delegations:
    //  - bob:     (40471 + 1) - 1234  = 39238
    //  - evan:    (40471 + 0) - 2345  = 38126
    //
    // Round 1: alice --(28948)--> bob
    // src_delegations:
    //  - charlie: 48416
    // dst_delegations:
    //  - bob:     39238 - 28948 = 10290
    //  - evan:    38126
    //
    // Round 2: charlie --(10290)--> bob
    // src_delegations:
    //  - charlie: 48416 - 10290 = 38126
    // dst_delegations:
    //  - evan:    38126
    //
    // Round 3: charlie --(38126)--> evan
    // Queues are emptied
    let expected = vec![
        Redelegation::new("alice", "bob", 28948, "uxyz"),
        Redelegation::new("charlie", "bob", 10290, "uxyz"),
        Redelegation::new("charlie", "evan", 38126, "uxyz"),
    ];

    assert_eq!(
        compute_redelegations_for_rebalancing(
            active_validators,
            &current_delegations,
            Uint128::from(10_u64),
            // mock the same mining power on every validator
            |_| Ok(40471_u128.into())
        )
        .unwrap(),
        expected,
    );

    let partially_active = vec![
        "alice".to_string(),
        "charlie".to_string(),
        "dave".to_string(),
        "evan".to_string(),
    ];

    let partially_expected = vec![
        Redelegation::new("alice", "dave", 10118, "uxyz"),
        Redelegation::new("alice", "evan", 8712, "uxyz"),
        Redelegation::new("charlie", "evan", 38299, "uxyz"),
    ];
    assert_eq!(
        compute_redelegations_for_rebalancing(
            partially_active.clone(),
            &current_delegations,
            Uint128::from(10_u64),
            // mock the same mining power on every validator
            |_| Ok(50589_u128.into())
        )
        .unwrap(),
        partially_expected,
    );

    let partially_expected_minimums = vec![
        Redelegation::new("alice", "evan", 18830, "uxyz"),
        Redelegation::new("charlie", "evan", 29414, "uxyz"),
    ];
    assert_eq!(
        compute_redelegations_for_rebalancing(
            partially_active,
            &current_delegations,
            Uint128::from(15_000_u64),
            // mock the same mining power on every validator
            |d| Ok(50589u128.into())
        )
        .unwrap(),
        partially_expected_minimums,
    );
}

#[test]
fn computing_redelegations_for_rebalancing_with_mining() {
    let current_delegations = vec![
        Delegation::new("alice", 69420, "uxyz"),
        Delegation::new("bob", 1234, "uxyz"),
        Delegation::new("charlie", 88888, "uxyz"),
        Delegation::new("dave", 40471, "uxyz"),
        Delegation::new("evan", 2345, "uxyz"),
    ];
    let total_delegated_amount = current_delegations.iter().map(|d| d.amount).sum::<u128>();
    let active_validators: Vec<String> = vec![
        "alice".to_string(),
        "bob".to_string(),
        "charlie".to_string(),
        "dave".to_string(),
        "evan".to_string(),
        // add steve to ensure still works for validators with no mining power
        "steve".to_string(),
    ];
    let mining_powers_by_validator = vec![
        ("alice".to_string(), 1002_u128),
        ("bob".to_string(), 3214_u128),
        ("charlie".to_string(), 881_u128),
        ("dave".to_string(), 5471_u128),
        ("evan".to_string(), 9285_u128),
    ];
    let total_mining_power = mining_powers_by_validator
        .iter()
        .map(|(_, power)| power)
        .sum::<u128>();

    // total delegated amount: 69420 + 1234 + 88888 + 40471 + 2345 = 202358
    // total mining power:         1002 + 3214 + 881 + 5471 + 9285 = 19853
    // remainder = 3
    //
    // alice target:                          202358 * 1002 / 19853 = 10213 + remainder 1 = 10214
    // bob target:                            202358 * 3214 / 19853 = 32759 + remainder 1 = 32760
    // charlie target:                         202358 * 881 / 19853 = 8979  + remainder 1 = 8980
    // dave target:                           202358 * 5471 / 19853 = 55764
    // evan target:                           202358 * 9285 / 19853 = 94640
    //
    // sum of targets:         10213 + 32759 + 8979 + 55764 + 94640 = 202355
    //
    // alice delta:                                   69420 - 10214 = 59206
    // bob delta:                                      1234 - 32760 = -31526
    // charlie delta:                                  88888 - 8980 = 79908
    // dave delta:                                    40471 - 55764 = -15293
    // evan delta:                                     2345 - 94640 = -92295
    //
    // sum of deltas:      59206 + -31526 + 79908 + -15293 + -92295 = 0
    //
    // Redelegations:
    // alice -> bob: 31526 (alice now has delta 27680)
    // alice -> dave: 15293 (alice now has delta 12387)
    // alice -> evan: 12387 (alice now has delta 0)
    // charlie -> evan: 79908 (charlie now has delta 0)

    let expected = vec![
        Redelegation::new("alice", "bob", 31526, "uxyz"),
        Redelegation::new("alice", "dave", 15293, "uxyz"),
        Redelegation::new("alice", "evan", 12387, "uxyz"),
        Redelegation::new("charlie", "evan", 79908, "uxyz"),
    ];

    assert_eq!(
        compute_redelegations_for_rebalancing(
            active_validators,
            &current_delegations,
            Uint128::from(10_u64),
            // mock the same mining power on every validator
            |d| compute_target_delegation_from_mining_power(
                total_delegated_amount.into(),
                mining_powers_by_validator
                    .iter()
                    .find(|(v, _)| v == &d.validator)
                    .unwrap()
                    .1
                    .into(),
                total_mining_power.into()
            )
            .into()
        )
        .unwrap(),
        expected,
        "round one mining weighted rebalancing"
    );

    let partially_active = vec![
        "alice".to_string(),
        "charlie".to_string(),
        "dave".to_string(),
        "evan".to_string(),
    ];

    let partially_expected = vec![
        Redelegation::new("alice", "dave", 10118, "uxyz"),
        Redelegation::new("alice", "evan", 8712, "uxyz"),
        Redelegation::new("charlie", "evan", 38299, "uxyz"),
    ];
    assert_eq!(
        compute_redelegations_for_rebalancing(
            partially_active.clone(),
            &current_delegations,
            Uint128::from(10_u64),
            // mock the same mining power on every validator
            |_| Ok(50589_u128.into())
        )
        .unwrap(),
        partially_expected,
        "round 2 mining weighted rebalancing"
    );

    let partially_expected_minimums = vec![
        Redelegation::new("alice", "evan", 18830, "uxyz"),
        Redelegation::new("charlie", "evan", 29414, "uxyz"),
    ];
    assert_eq!(
        compute_redelegations_for_rebalancing(
            partially_active,
            &current_delegations,
            Uint128::from(15_000_u64),
            // mock the same mining power on every validator
            |d| Ok(50589u128.into())
        )
        .unwrap(),
        partially_expected_minimums,
        "round 2 mining weighted rebalancing with minimums"
    );
}

//--------------------------------------------------------------------------------------------------
// Coins
//--------------------------------------------------------------------------------------------------

#[test]
fn parsing_coin() {
    let coin = parse_coin("12345uatom").unwrap();
    assert_eq!(coin, Coin::new(12345, "uatom"));

    let coin =
        parse_coin("23456ibc/0471F1C4E7AFD3F07702BEF6DC365268D64570F7C1FDC98EA6098DD6DE59817B")
            .unwrap();
    assert_eq!(
        coin,
        Coin::new(
            23456,
            "ibc/0471F1C4E7AFD3F07702BEF6DC365268D64570F7C1FDC98EA6098DD6DE59817B"
        )
    );

    let err = parse_coin("69420").unwrap_err();
    assert_eq!(err, StdError::generic_err("failed to parse coin: 69420"));

    let err = parse_coin("ngmi").unwrap_err();
    assert_eq!(
        err,
        StdError::generic_err("Parsing u128: cannot parse integer from empty string")
    );
}

#[test]
fn parsing_coins() {
    let coins = Coins::from_str("").unwrap();
    assert_eq!(coins.0, vec![]);

    let coins = Coins::from_str("12345uatom").unwrap();
    assert_eq!(coins.0, vec![Coin::new(12345, "uatom")]);

    let coins = Coins::from_str("12345uatom,23456uxyz").unwrap();
    assert_eq!(
        coins.0,
        vec![Coin::new(12345, "uatom"), Coin::new(23456, "uxyz")]
    );
}

#[test]
fn adding_coins() {
    let mut coins = Coins(vec![]);

    coins.add(&Coin::new(12345, "uatom")).unwrap();
    assert_eq!(coins.0, vec![Coin::new(12345, "uatom")]);

    coins.add(&Coin::new(23456, "uxyz")).unwrap();
    assert_eq!(
        coins.0,
        vec![Coin::new(12345, "uatom"), Coin::new(23456, "uxyz")]
    );

    coins
        .add_many(&Coins::from_str("76543uatom,69420uusd").unwrap())
        .unwrap();
    assert_eq!(
        coins.0,
        vec![
            Coin::new(88888, "uatom"),
            Coin::new(23456, "uxyz"),
            Coin::new(69420, "uusd")
        ]
    );
}

#[test]
fn receiving_funds() {
    let err = parse_received_fund(&[], "uxyz").unwrap_err();
    assert_eq!(
        err,
        StdError::generic_err("must deposit exactly one coin; received 0")
    );

    let err = parse_received_fund(
        &[Coin::new(12345, "uatom"), Coin::new(23456, "uxyz")],
        "uxyz",
    )
    .unwrap_err();
    assert_eq!(
        err,
        StdError::generic_err("must deposit exactly one coin; received 2")
    );

    let err = parse_received_fund(&[Coin::new(12345, "uatom")], "uxyz").unwrap_err();
    assert_eq!(
        err,
        StdError::generic_err("expected uxyz deposit, received uatom")
    );

    let err = parse_received_fund(&[Coin::new(0, "uxyz")], "uxyz").unwrap_err();
    assert_eq!(
        err,
        StdError::generic_err("deposit amount must be non-zero")
    );

    let amount = parse_received_fund(&[Coin::new(69420, "uxyz")], "uxyz").unwrap();
    assert_eq!(amount, Uint128::new(69420));
}
