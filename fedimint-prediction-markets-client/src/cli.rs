use std::collections::{HashMap, HashSet};
use std::str::FromStr;
use std::{ffi, iter};

use anyhow::bail;
use clap::Parser;
use fedimint_core::{Amount, TransactionId};
use fedimint_prediction_markets_common::{
    ContractOfOutcomeAmount, PredictionMarketEventHashHex, PredictionMarketEventJson, Seconds,
    Side, UnixTimestamp, WeightRequiredForPayout,
};
use prediction_market_event::Outcome;
use prediction_market_event_nostr_client::nostr_sdk::JsonUtil;
use serde::Serialize;
use serde_json::json;

use crate::order_filter::{self};
use crate::{market_outpoint_from_tx_id, OrderId, PredictionMarketsClientModule};

#[derive(Parser, Serialize)]
enum Opts {
    NewMarket {
        event_hash_hex: PredictionMarketEventHashHex,
        contract_price: Amount,
        payout_control: prediction_market_event_nostr_client::nostr_sdk::nostr::PublicKey,
    },
    GetMarket {
        market_txid: TransactionId,
        #[clap(short, long, default_value = "false")]
        from_local_cache: bool,
    },
    PayoutMarket {
        market_txid: TransactionId,
    },
    GetEventPayoutAttestationsUsedToPermitPayout {
        market_txid: TransactionId,
    },
    NewOrder {
        market_txid: TransactionId,
        outcome: Outcome,
        side: Side,
        price: Amount,
        quantity: ContractOfOutcomeAmount,
    },
    GetOrder {
        id: OrderId,
        #[clap(short, long, default_value = "false")]
        from_local_cache: bool,
    },
    CancelOrder {
        id: OrderId,
    },
    WithdrawAvailableBitcoin,
    SyncPayouts {
        #[clap(short, long)]
        market_txid: Option<TransactionId>,
    },
    ListOrders {
        #[clap(short, long)]
        market_txid: Option<TransactionId>,
        #[clap(short, long)]
        outcome: Option<Outcome>,
    },
    RecoverOrders {
        #[clap(short, long)]
        gap_size_to_check: Option<usize>,
    },
    GetCandlesticks {
        market_txid: TransactionId,
        outcome: Outcome,
        candlestick_interval: Seconds,
        min_candlestick_timestamp: UnixTimestamp,
    },
}

pub async fn handle_cli_command(
    prediction_markets: &PredictionMarketsClientModule,
    args: &[ffi::OsString],
) -> anyhow::Result<serde_json::Value> {
    let opts =
        Opts::parse_from(iter::once(&ffi::OsString::from("prediction-markets")).chain(args.iter()));

    let value = match opts {
        Opts::NewMarket {
            event_hash_hex,
            contract_price,
            payout_control,
        } => {
            let payout_control_weight_map =
                vec![(payout_control.to_hex(), 1u16)].into_iter().collect();
            let weight_required_for_payout = 1;

            if !prediction_market_event::EventHashHex::is_valid_format(&event_hash_hex) {
                bail!("event_hash_hex: invalid format")
            }
            let nostr_client = get_nostr_client().await?;
            let Some((_, event)) = nostr_client
                .get::<prediction_market_event_nostr_client::prediction_market_event::nostr_event_types::NewEvent>(|f| vec![f.hashtag(event_hash_hex)], None)
                .await?
                .into_iter()
                .next()
            else {
                bail!("could not find event on nostr")
            };
            let event_json = event.try_to_json_string()?;

            let res = prediction_markets
                .new_market(
                    event_json,
                    contract_price,
                    payout_control_weight_map,
                    weight_required_for_payout,
                )
                .await?
                .txid;
            json!(res)
        }
        Opts::GetMarket {
            market_txid,
            from_local_cache,
        } => {
            let res = prediction_markets
                .get_market(market_outpoint_from_tx_id(market_txid), from_local_cache)
                .await?;
            json!(res)
        }
        Opts::PayoutMarket { market_txid } => {
            let Some(market) = prediction_markets
                .get_market(market_outpoint_from_tx_id(market_txid), false)
                .await?
            else {
                bail!("market does not exist")
            };
            let event_hash_hex = market.0.event()?.hash_hex()?;
            let nostr_client = get_nostr_client().await?;
            let event_payout_attestation_result = nostr_client.get::<prediction_market_event_nostr_client::prediction_market_event::nostr_event_types::EventPayoutAttestation>(|f| {
                market.0.payout_control_weight_map.iter().map(|(pk, _)| {
                    let author = prediction_market_event_nostr_client::nostr_sdk::PublicKey::parse(pk).unwrap();
                    f.clone().author(author).hashtag(&event_hash_hex.0)
                }).collect()
            }, None).await?;
            let mut seen_payout_controls: HashSet<
                prediction_market_event_nostr_client::prediction_market_event::nostr_event_types::NostrPublicKeyHex
            > = HashSet::new();
            let mut event_payout_stats: HashMap<
                prediction_market_event_nostr_client::prediction_market_event::EventPayout,
                (Vec<PredictionMarketEventJson>, WeightRequiredForPayout),
            > = HashMap::new();

            for (nostr_event, (payout_control, event_payout)) in event_payout_attestation_result {
                let Some(weight) = market.0.payout_control_weight_map.get(&payout_control.0) else {
                    continue;
                };
                if !seen_payout_controls.insert(payout_control) {
                    continue;
                }
                if !event_payout_stats.contains_key(&event_payout) {
                    event_payout_stats.insert(event_payout.clone(), (Vec::new(), 0));
                }

                let event_payout_stats_value = event_payout_stats.get_mut(&event_payout).unwrap();
                event_payout_stats_value.0.push(nostr_event.try_as_json()?);
                event_payout_stats_value.1 += WeightRequiredForPayout::from(*weight);
            }
            let mut found_payout = None;
            for (event_payout, (event_payout_attestations_json, total_weight)) in event_payout_stats
            {
                if market.0.weight_required_for_payout > total_weight {
                    continue;
                }

                found_payout = Some((event_payout, event_payout_attestations_json));
                break;
            }

            match found_payout {
                Some((event_payout, event_payout_attestations_json)) => {
                    prediction_markets
                        .payout_market(
                            market_outpoint_from_tx_id(market_txid),
                            event_payout_attestations_json,
                        )
                        .await?;

                    json!({
                        "payout_submitted": true,
                        "event_payout": event_payout
                    })
                }
                None => {
                    json!({
                        "payout_submitted": false,
                    })
                }
            }
        }
        Opts::GetEventPayoutAttestationsUsedToPermitPayout { market_txid } => {
            let res = prediction_markets
                .get_event_payout_attestations_used_to_permit_payout(market_outpoint_from_tx_id(
                    market_txid,
                ))
                .await?;

            json!(res)
        }

        Opts::NewOrder {
            market_txid,
            outcome,
            side,
            price,
            quantity,
        } => {
            let res = prediction_markets
                .new_order(
                    market_outpoint_from_tx_id(market_txid),
                    outcome,
                    side,
                    price,
                    quantity,
                )
                .await?;

            json!(res)
        }
        Opts::GetOrder {
            id,
            from_local_cache,
        } => {
            let res = prediction_markets.get_order(id, from_local_cache).await?;

            json!(res)
        }
        Opts::CancelOrder { id } => {
            let res = prediction_markets.cancel_order(id).await?;

            json!(res)
        }
        Opts::WithdrawAvailableBitcoin => {
            let res = prediction_markets
                .send_order_bitcoin_balance_to_primary_module()
                .await?;

            json!(res)
        }
        Opts::SyncPayouts { market_txid } => {
            let res = prediction_markets
                .sync_payouts(market_txid.map(|v| market_outpoint_from_tx_id(v)))
                .await?;

            json!(res)
        }
        Opts::ListOrders {
            market_txid,
            outcome,
        } => {
            let order_path = match market_txid {
                None => order_filter::OrderPath::All,
                Some(market_txid) => match outcome {
                    None => order_filter::OrderPath::Market {
                        market: market_outpoint_from_tx_id(market_txid),
                    },
                    Some(outcome) => order_filter::OrderPath::MarketOutcome {
                        market: market_outpoint_from_tx_id(market_txid),
                        outcome,
                    },
                },
            };
            let res = prediction_markets
                .get_orders_from_db(order_filter::OrderFilter(
                    order_path,
                    order_filter::OrderState::Any,
                ))
                .await;

            json!(res)
        }
        Opts::RecoverOrders { gap_size_to_check } => {
            let res = prediction_markets
                .resync_order_slots(gap_size_to_check.unwrap_or(25))
                .await?;

            json!(res)
        }
        Opts::GetCandlesticks {
            market_txid,
            outcome,
            candlestick_interval,
            min_candlestick_timestamp,
        } => {
            let res = prediction_markets
                .get_candlesticks(
                    market_outpoint_from_tx_id(market_txid),
                    outcome,
                    candlestick_interval,
                    min_candlestick_timestamp,
                )
                .await?;

            json!(res)
        }
    };

    Ok(value)
}

const RECOMMENDED_RELAY_LIST: &[&str] = &[
    "wss://btc.klendazu.com",
    "wss://nostr.yael.at",
    "wss://nostr.oxtr.dev",
    "wss://relay.lexingtonbitcoin.org",
    "wss://nos.lol",
    "wss://nostr.bitcoiner.social",
    "wss://relay.primal.net",
    "wss://nostrrelay.com",
];

async fn get_nostr_client() -> anyhow::Result<prediction_market_event_nostr_client::Client> {
    let relays = RECOMMENDED_RELAY_LIST
        .iter()
        .map(|s| prediction_market_event_nostr_client::nostr_sdk::Url::from_str(s).unwrap())
        .collect();
    let client =
        prediction_market_event_nostr_client::Client::new_initialized_client_query_only(relays)
            .await?;

    Ok(client)
}
