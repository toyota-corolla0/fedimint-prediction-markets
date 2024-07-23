use fedimint_client::sm::{DynState, State, StateTransition};
use fedimint_client::DynGlobalClientContext;
use fedimint_core::core::{IntoDynInstance, ModuleInstanceId, OperationId};
use fedimint_core::encoding::{Decodable, Encodable};
use fedimint_core::TransactionId;
use fedimint_prediction_markets_common::OrderIdClientSide;

// use serde::{Deserialize, Serialize};
// use thiserror::Error;
use crate::{PredictionMarketsClientContext, PredictionMarketsClientModule};

/// Tracks a transaction.
#[derive(Debug, Clone, Eq, PartialEq, Hash, Decodable, Encodable)]
pub struct PredictionMarketsStateMachine {
    pub operation_id: OperationId,
    pub state: PredictionMarketState,
}

#[derive(Debug, Clone, Eq, PartialEq, Hash, Decodable, Encodable)]
pub enum PredictionMarketState {
    NewMarket {
        tx_id: TransactionId,
    },
    NewMarketAccepted,
    NewMarketFailed,

    ProposePayout {
        tx_id: TransactionId,
    },
    ProposePayoutAccepted,
    ProposePayoutFailed,

    NewOrder {
        tx_id: TransactionId,
        order: OrderIdClientSide,
        sources: Vec<OrderIdClientSide>,
    },
    NewOrderAccepted,
    NewOrderFailed,

    CancelOrder {
        tx_id: TransactionId,
        order: OrderIdClientSide,
    },
    CancelOrderAccepted,
    CancelOrderFailed,

    ConsumeOrderBitcoinBalance {
        tx_id: TransactionId,
        order: OrderIdClientSide,
    },
    ConsumeOrderBitcoinBalanceAccepted,
    ConsumeOrderBitcoinBalanceFailed,

    ConsumePayoutControlBitcoinBalance {
        tx_id: TransactionId,
    },
    ConsumePayoutControlBitcoinBalanceAccepted,
    ConsumePayoutControlBitcoinBalanceFailed,
}

impl State for PredictionMarketsStateMachine {
    type ModuleContext = PredictionMarketsClientContext;

    fn transitions(
        &self,
        _context: &Self::ModuleContext,
        global_context: &DynGlobalClientContext,
    ) -> Vec<StateTransition<Self>> {
        let operation_id = self.operation_id;

        match self.state.clone() {
            PredictionMarketState::NewMarket { tx_id } => {
                vec![StateTransition::new(
                    await_tx_accepted(global_context.clone(), operation_id, tx_id),
                    move |_dbtx, res, _state_machine: Self| match res {
                        // tx accepted
                        Ok(_) => Box::pin(async move {
                            Self {
                                operation_id,
                                state: PredictionMarketState::NewMarketAccepted,
                            }
                        }),
                        // tx rejected
                        Err(_) => Box::pin(async move {
                            Self {
                                operation_id,
                                state: PredictionMarketState::NewMarketFailed,
                            }
                        }),
                    },
                )]
            }
            PredictionMarketState::NewMarketAccepted => vec![],
            PredictionMarketState::NewMarketFailed => vec![],

            PredictionMarketState::ProposePayout { tx_id } => {
                vec![StateTransition::new(
                    await_tx_accepted(global_context.clone(), operation_id, tx_id),
                    move |_dbtx, res, _state: Self| match res {
                        // tx accepted
                        Ok(_) => Box::pin(async move {
                            Self {
                                operation_id,
                                state: PredictionMarketState::ProposePayoutAccepted,
                            }
                        }),
                        // tx rejected
                        Err(_) => Box::pin(async move {
                            Self {
                                operation_id,
                                state: PredictionMarketState::ProposePayoutFailed,
                            }
                        }),
                    },
                )]
            }
            PredictionMarketState::ProposePayoutAccepted => vec![],
            PredictionMarketState::ProposePayoutFailed => vec![],

            PredictionMarketState::NewOrder {
                tx_id,
                order,
                sources,
            } => {
                vec![StateTransition::new(
                    await_tx_accepted(global_context.clone(), operation_id, tx_id),
                    move |dbtx, res, _state: Self| match res {
                        // tx accepted
                        Ok(_) => {
                            let mut changed_orders = Vec::new();
                            changed_orders.push(order);
                            changed_orders.append(&mut sources.clone());

                            Box::pin(async move {
                                PredictionMarketsClientModule::set_order_needs_update(
                                    dbtx.module_tx(),
                                    changed_orders,
                                )
                                .await;
                                Self {
                                    operation_id,
                                    state: PredictionMarketState::NewOrderAccepted,
                                }
                            })
                        }
                        // tx rejected
                        Err(_) => Box::pin(async move {
                            PredictionMarketsClientModule::unreserve_order_id_slot(
                                dbtx.module_tx(),
                                order,
                            )
                            .await;
                            Self {
                                operation_id,
                                state: PredictionMarketState::NewOrderFailed,
                            }
                        }),
                    },
                )]
            }
            PredictionMarketState::NewOrderAccepted => vec![],
            PredictionMarketState::NewOrderFailed => vec![],

            PredictionMarketState::CancelOrder { tx_id, order } => {
                vec![StateTransition::new(
                    await_tx_accepted(global_context.clone(), operation_id, tx_id),
                    move |dbtx, res, _state: Self| match res {
                        // tx accepted
                        Ok(_) => Box::pin(async move {
                            PredictionMarketsClientModule::set_order_needs_update(
                                dbtx.module_tx(),
                                vec![order],
                            )
                            .await;
                            Self {
                                operation_id,
                                state: PredictionMarketState::CancelOrderAccepted,
                            }
                        }),
                        // tx rejected
                        Err(_) => Box::pin(async move {
                            Self {
                                operation_id,
                                state: PredictionMarketState::CancelOrderFailed,
                            }
                        }),
                    },
                )]
            }
            PredictionMarketState::CancelOrderAccepted => vec![],
            PredictionMarketState::CancelOrderFailed => vec![],

            PredictionMarketState::ConsumeOrderBitcoinBalance { tx_id, order } => {
                vec![StateTransition::new(
                    await_tx_accepted(global_context.clone(), operation_id, tx_id),
                    move |dbtx, res, _state: Self| match res {
                        // tx accepted
                        Ok(_) => Box::pin(async move {
                            PredictionMarketsClientModule::set_order_needs_update(
                                dbtx.module_tx(),
                                vec![order],
                            )
                            .await;
                            Self {
                                operation_id,
                                state: PredictionMarketState::ConsumeOrderBitcoinBalanceAccepted,
                            }
                        }),
                        // tx rejected
                        Err(_) => Box::pin(async move {
                            Self {
                                operation_id,
                                state: PredictionMarketState::ConsumeOrderBitcoinBalanceFailed,
                            }
                        }),
                    },
                )]
            }
            PredictionMarketState::ConsumeOrderBitcoinBalanceAccepted => vec![],
            PredictionMarketState::ConsumeOrderBitcoinBalanceFailed => vec![],

            PredictionMarketState::ConsumePayoutControlBitcoinBalance { tx_id } => {
                vec![StateTransition::new(
                    await_tx_accepted(global_context.clone(), operation_id, tx_id),
                    move |_dbtx, res, _state: Self| match res {
                        // tx accepted
                        Ok(_) => Box::pin(async move {
                            Self {
                                operation_id,
                                state: PredictionMarketState::ConsumePayoutControlBitcoinBalanceAccepted,
                            }
                        }),
                        // tx rejected
                        Err(_) => Box::pin(async move {
                            Self {
                                operation_id,
                                state:
                                    PredictionMarketState::ConsumePayoutControlBitcoinBalanceFailed,
                            }
                        }),
                    },
                )]
            }
            PredictionMarketState::ConsumePayoutControlBitcoinBalanceAccepted => vec![],
            PredictionMarketState::ConsumePayoutControlBitcoinBalanceFailed => vec![],
        }
    }

    fn operation_id(&self) -> OperationId {
        self.operation_id
    }
}

// TODO: Boiler-plate, should return OutputOutcome
async fn await_tx_accepted(
    context: DynGlobalClientContext,
    _id: OperationId,
    txid: TransactionId,
) -> Result<(), String> {
    context.await_tx_accepted(txid).await
}

// TODO: Boiler-plate
impl IntoDynInstance for PredictionMarketsStateMachine {
    type DynType = DynState;

    fn into_dyn(self, instance_id: ModuleInstanceId) -> Self::DynType {
        DynState::from_typed(instance_id, self)
    }
}
