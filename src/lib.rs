mod blockstore;

use crate::blockstore::Blockstore;
use cid::multihash::Code;
use cid::Cid;
use fvm_ipld_encoding::tuple::{Deserialize_tuple, Serialize_tuple};
use fvm_ipld_encoding::{to_vec, CborStore, Cbor, RawBytes, DAG_CBOR, from_slice};
use fvm_sdk as sdk;
use fvm_sdk::message::{params_raw, NO_DATA_BLOCK_ID};
use fvm_shared::ActorID;
use fvm_shared::econ::TokenAmount;
use fvm_shared::bigint::{bigint_ser};
use fvm_shared::bigint::bigint_ser::{BigIntDe};
use fvm_shared::address::Address;
use fvm_ipld_hamt::Hamt;


/// A macro to abort concisely.
/// This should be part of the SDK as it's very handy.
macro_rules! abort {
    ($code:ident, $msg:literal $(, $ex:expr)*) => {
        fvm_sdk::vm::abort(
            fvm_shared::error::ExitCode::$code.value(),
            Some(format!($msg, $($ex,)*).as_str()),
        )
    };
}

/// The state object.
#[derive(Serialize_tuple, Deserialize_tuple, Clone, Debug)]
pub struct State {
    pub name: String,
    pub symbol: String,
    #[serde(with = "bigint_ser")]
    pub max_supply: TokenAmount,
    pub owner: Address,
    pub balances: Cid,
}

/// We should probably have a derive macro to mark an object as a state object,
/// and have load and save methods automatically generated for them as part of a
/// StateObject trait (i.e. impl StateObject for State).
impl State {
    pub fn load() -> Self {
        // First, load the current state root.
        let root = match sdk::sself::root() {
            Ok(root) => root,
            Err(err) => abort!(USR_ILLEGAL_STATE, "failed to get root: {:?}", err),
        };

        // Load the actor state from the state tree.
        match Blockstore.get_cbor::<Self>(&root) {
            Ok(Some(state)) => state,
            Ok(None) => abort!(USR_ILLEGAL_STATE, "state does not exist"),
            Err(err) => abort!(USR_ILLEGAL_STATE, "failed to get state: {}", err),
        }
    }

    pub fn save(&self) -> Cid {
        let serialized = match to_vec(self) {
            Ok(s) => s,
            Err(err) => abort!(USR_SERIALIZATION, "failed to serialize state: {:?}", err),
        };
        let cid = match sdk::ipld::put(Code::Blake2b256.into(), 32, DAG_CBOR, serialized.as_slice())
        {
            Ok(cid) => cid,
            Err(err) => abort!(USR_SERIALIZATION, "failed to store initial state: {:}", err),
        };
        if let Err(err) = sdk::sself::set_root(&cid) {
            abort!(USR_ILLEGAL_STATE, "failed to set root ciid: {:}", err);
        }
        cid
    }

    pub fn new(p: ConstructorParams) -> State {
        let mut balances : Hamt<Blockstore, BigIntDe, ActorID> = Hamt::new(Blockstore);

        let balances = match balances.flush() {
            Ok(map) => map,
            Err(_e) => abort!(USR_ILLEGAL_STATE, "failed to create balances hamt"),
        };

        State {
            name: p.name,
            symbol: p.symbol,
            max_supply: p.max_supply,
            owner: p.owner,
            balances
        }
    }
}

/// The actor's WASM entrypoint. It takes the ID of the parameters block,
/// and returns the ID of the return value block, or NO_DATA_BLOCK_ID if no
/// return value.
///
/// Should probably have macros similar to the ones on fvm.filecoin.io snippets.
/// Put all methods inside an impl struct and annotate it with a derive macro
/// that handles state serde and dispatch.
#[no_mangle]
pub fn invoke(params_id: u32) -> u32 {
    // Conduct method dispatch. Handle input parameters and return data.
    let ret: Option<RawBytes> = match sdk::message::method_number() {
        1 => {
            let params: ConstructorParams = match params_raw(params_id) {
                Ok(params) => {
                    match from_slice(params.1.as_slice()) {
                        Ok(v) => v,
                        Err(err) => abort!(USR_SERIALIZATION, "failed to parse params: {:?}", err),
                    }
                },
                Err(err) => abort!(USR_ILLEGAL_ARGUMENT, "failed to parse address: {:?}", err),
            };
            constructor(params);
            None
        },
        2 => {
            let params: TransferParams = match params_raw(params_id) {
                Ok(params) => {
                    match from_slice(params.1.as_slice()) {
                        Ok(v) => v,
                        Err(err) => abort!(USR_SERIALIZATION, "failed to parse params: {:?}", err),
                    }
                },
                Err(err) => abort!(USR_ILLEGAL_ARGUMENT, "failed to parse params: {:?}", err),
            };
            mint(params);
            None
        },
        3 => {
            let params: TransferParams = match params_raw(params_id) {
                Ok(params) => {
                    match from_slice(params.1.as_slice()) {
                        Ok(v) => v,
                        Err(err) => abort!(USR_SERIALIZATION, "failed to parse params: {:?}", err),
                    }
                },
                Err(err) => abort!(USR_ILLEGAL_ARGUMENT, "failed to parse params: {:?}", err),
            };
            transfer(params);
            None
        }
        _ => abort!(USR_UNHANDLED_MESSAGE, "unrecognized method"),
    };

    // Insert the return data block if necessary, and return the correct
    // block ID.
    match ret {
        None => NO_DATA_BLOCK_ID,
        Some(v) => match sdk::ipld::put_block(DAG_CBOR, v.bytes()) {
            Ok(id) => id,
            Err(err) => abort!(USR_SERIALIZATION, "failed to store return value: {}", err),
        },
    }
}

// hGRHTElGY0dMRkQAD0JAQgBk
#[derive(Serialize_tuple, Deserialize_tuple)]
pub struct ConstructorParams {
    pub name: String,
    pub symbol: String,
    #[serde(with = "bigint_ser")]
    pub max_supply: TokenAmount,
    pub owner: Address,
}

/// The constructor populates the initial state.
///
/// Method num 1. This is part of the Filecoin calling convention.
/// InitActor#Exec will call the constructor on method_num = 1.
pub fn constructor(params: ConstructorParams) -> Option<RawBytes> {
    // This constant should be part of the SDK.
    const INIT_ACTOR_ADDR: ActorID = 1;

    // Should add SDK sugar to perform ACL checks more succinctly.
    // i.e. the equivalent of the validate_* builtin-actors runtime methods.
    // https://github.com/filecoin-project/builtin-actors/blob/master/actors/runtime/src/runtime/fvm.rs#L110-L146
    if sdk::message::caller() != INIT_ACTOR_ADDR {
        abort!(USR_FORBIDDEN, "constructor invoked by non-init actor");
    }

    let state = State::new(params);
    state.save();
    None
}

pub fn mint(params: TransferParams) {
    let mut state = State::load();

    // Resolve the recipient into an ID address.
    // TODO See addressing section on module docs.
    let owner_id = match fvm_sdk::actor::resolve_address(&state.owner) {
        Some(id) => id,
        None => abort!(USR_ILLEGAL_ARGUMENT, "failed to resolve address"),
    };

    if owner_id != fvm_sdk::message::caller() {
        abort!(USR_FORBIDDEN, "mint invoked by non-owner");
    }

    // Load the balances HAMT.
    // TODO Using BitIntDe because it's both Ser and De; this is a misnomer and
    //  we should fix it.
    let mut balances =
        match Hamt::<Blockstore, BigIntDe, ActorID>::load(&state.balances, Blockstore) {
            Ok(map) => map,
            Err(err) => abort!(USR_ILLEGAL_STATE, "failed to load balances hamt: {:?}", err),
        };
}

/// The input parameters for a transfer.
#[derive(Serialize_tuple, Deserialize_tuple, Clone, Debug)]
pub struct TransferParams {
    pub recipient: Address,
    #[serde(with = "bigint_ser")]
    pub amount: TokenAmount,
}

impl Cbor for TransferParams {}

/// Transfer a token amount.
pub fn transfer(params: TransferParams) {
    let mut state = State::load();

    // Load the balances HAMT.
    // TODO Using BitIntDe because it's both Ser and De; this is a misnomer and
    //  we should fix it.
    let mut balances =
        match Hamt::<Blockstore, BigIntDe, ActorID>::load(&state.balances, Blockstore) {
            Ok(map) => map,
            Err(err) => abort!(USR_ILLEGAL_STATE, "failed to load balances hamt: {:?}", err),
        };

    // Load the sender's balance.
    let sender_id = fvm_sdk::message::caller();
    let mut sender_bal = match balances.get(&sender_id) {
        Ok(Some(bal)) => bal.clone(),
        Ok(None) => BigIntDe(TokenAmount::from(0)),
        Err(err) => abort!(USR_ILLEGAL_STATE, "failed to get balance: {:?}", err),
    };

    // Sender has insufficient balance.
    if sender_bal.0 < params.amount {
        abort!(USR_INSUFFICIENT_FUNDS, "sender has insufficient balance")
    }

    // Resolve the recipient into an ID address.
    // TODO See addressing section on module docs.
    let recipient_id = match fvm_sdk::actor::resolve_address(&params.recipient) {
        Some(id) => id,
        None => abort!(USR_ILLEGAL_ARGUMENT, "failed to resolve address"),
    };

    // Forbid sends to self.
    if sender_id == recipient_id {
        abort!(USR_ILLEGAL_ARGUMENT, "cannot send to self");
    }

    // // Ensure that the recipient is an account actor; otherwise they will never
    // // be able to spend the funds. (At least in the current protocol)
    // match sdk::actor::get_actor_code_cid(&params.recipient) {
    //     None => abort!(USR_ILLEGAL_ARGUMENT, "cannot resolve actor type of recipient"),
    //     Some(cid) => {
    //         /// The multicodec value for raw data.
    //         const IPLD_CODEC_RAW: u64 = 0x55;
    //         // TODO this is embarrassingly wrong, as hardcoding the actor version doesn't allow evolution.
    //         //  but we need to figure out the upgradability story to solve this.
    //         let other = Cid::new_v1(IPLD_CODEC_RAW, Code::Identity.digest(b"fil/6/account"));
    //         if cid != other {
    //             abort!(USR_ILLEGAL_ARGUMENT, "recipient is not an account actor")
    //         }
    //     }
    // }

    // Load the recipient's balance.
    let mut recipient_bal = match balances.get(&recipient_id) {
        Ok(Some(bal)) => bal.clone(),
        Ok(None) => BigIntDe(TokenAmount::from(0)),
        Err(err) => abort!(
            USR_ILLEGAL_STATE,
            "failed to query hamt when getting recipient balance: {:?}",
            err
        ),
    };

    // Update balances.
    sender_bal.0 -= &params.amount;
    recipient_bal.0 += &params.amount;

    // Set the updated sender balance in the balances HAMT.
    if let Err(err) = balances.set(sender_id, sender_bal.clone()) {
        abort!(
            USR_ILLEGAL_STATE,
            "failed to set new sender balance in balances hamt: {:?}",
            err
        )
    }

    // Set the updated recipient balance in the balances HAMT.
    if let Err(err) = balances.set(recipient_id, recipient_bal.clone()) {
        abort!(
            USR_ILLEGAL_STATE,
            "failed to set new recipient balance in balances hamt: {:?}",
            err
        )
    }

    // Flush the HAMT to generate the new root CID to update the actor's state.
    let cid = match balances.flush() {
        Ok(cid) => cid,
        Err(err) => abort!(
            USR_ILLEGAL_STATE,
            "failed to query hamt when getting recipient balance: {:?}",
            err
        ),
    };

    // Update the actor's state.
    state.balances = cid;
    let root = match Blockstore.put_cbor(&state, Code::Blake2b256) {
        Ok(cid) => cid,
        Err(err) => abort!(USR_ILLEGAL_STATE, "failed to write new state: {:?}", err),
    };

    if let Err(err) = fvm_sdk::sself::set_root(&root) {
        abort!(USR_ILLEGAL_STATE, "failed to set new state root: {:?}", err)
    }
}


