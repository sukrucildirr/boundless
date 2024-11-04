// Copyright (c) 2024 RISC Zero, Inc.
//
// All rights reserved.

use std::borrow::Cow;
#[cfg(not(target_os = "zkvm"))]
use std::str::FromStr;

#[cfg(not(target_os = "zkvm"))]
use alloy::{
    contract::Error as ContractErr,
    primitives::SignatureError,
    signers::{Error as SignerErr, Signature, SignerSync},
    sol_types::{Error as DecoderErr, SolInterface, SolStruct},
    transports::TransportError,
};
use alloy_primitives::{
    aliases::{U160, U192, U96},
    Address, Bytes, B256, U256,
};
use alloy_sol_types::{eip712_domain, Eip712Domain};
use serde::{Deserialize, Serialize};
#[cfg(not(target_os = "zkvm"))]
use std::time::Duration;
#[cfg(not(target_os = "zkvm"))]
use thiserror::Error;
use url::Url;

use risc0_zkvm::sha::Digest;

#[cfg(not(target_os = "zkvm"))]
pub use risc0_ethereum_contracts::encode_seal;

#[cfg(not(target_os = "zkvm"))]
const TXN_CONFIRM_TIMEOUT: Duration = Duration::from_secs(45);

// proof_market.rs is a copy of IProofMarket.sol with alloy derive statements added.
// See the build.rs script in this crate for more details.
include!(concat!(env!("OUT_DIR"), "/proof_market.rs"));

/// Status of a proving request
#[derive(Debug, PartialEq)]
pub enum ProofStatus {
    /// The request has expired.
    Expired,
    /// The request is locked in and waiting for fulfillment.
    Locked,
    /// The request has been fulfilled.
    Fulfilled,
    /// The request has an unknown status.
    ///
    /// This is used to represent the status of a request
    /// with no evidence in the state. The request may be
    /// open for bidding or it may not exist.
    Unknown,
}

#[derive(Deserialize, Serialize, Clone, Debug, PartialEq)]
pub struct EIP721DomainSaltless {
    pub name: Cow<'static, str>,
    pub version: Cow<'static, str>,
    pub chain_id: u64,
    pub verifying_contract: Address,
}

impl EIP721DomainSaltless {
    pub fn alloy_struct(&self) -> Eip712Domain {
        eip712_domain! {
            name: self.name.clone(),
            version: self.version.clone(),
            chain_id: self.chain_id,
            verifying_contract: self.verifying_contract,
        }
    }
}

pub(crate) fn request_id(addr: &Address, id: u32) -> U192 {
    let addr = U160::try_from(*addr).unwrap();
    (U192::from(addr) << 32) | U192::from(id)
}

impl ProvingRequest {
    /// Creates a new proving request with the given parameters.
    ///
    /// The request ID is generated by combining the address and given idx.
    pub fn new(
        idx: u32,
        addr: &Address,
        requirements: Requirements,
        image_url: &str,
        input: Input,
        offer: Offer,
    ) -> Self {
        Self {
            id: U192::from(request_id(addr, idx)),
            requirements,
            imageUrl: image_url.to_string(),
            input,
            offer,
        }
    }

    /// Returns the client address from the proving request ID.
    pub fn client_address(&self) -> Address {
        let shifted_id: U256 = U256::from(self.id) >> 32;
        let shifted_bytes: [u8; 32] = shifted_id.to_be_bytes();
        let addr_bytes: [u8; 20] =
            shifted_bytes[12..32].try_into().expect("Failed to extract address bytes");
        let lower_160_bits = U160::from_be_bytes(addr_bytes);

        Address::from(lower_160_bits)
    }

    /// Sets the input data to be fetched from the given URL.
    pub fn with_image_url(self, image_url: &str) -> Self {
        Self { imageUrl: image_url.to_string(), ..self }
    }

    /// Sets the requirements for the proving request.
    pub fn with_requirements(self, requirements: Requirements) -> Self {
        Self { requirements, ..self }
    }

    /// Sets the guest's input for the proving request.
    pub fn with_input(self, input: Input) -> Self {
        Self { input, ..self }
    }

    /// Sets the offer for the proving request.
    pub fn with_offer(self, offer: Offer) -> Self {
        Self { offer, ..self }
    }

    /// Returns the block number at which the request expires.
    pub fn expires_at(&self) -> u64 {
        self.offer.biddingStart + self.offer.timeout as u64
    }

    /// Check that the request is valid and internally consistent.
    ///
    /// If any field are empty, or if two fields conflict (e.g. the max price is less than the min
    /// price) this function will return an error.
    // TODO: Should this function be replaced with a proper builder that checks the ProvingRequest
    // before it finalizes the construction?
    #[cfg_attr(target_os = "zkvm", allow(dead_code))]
    pub(crate) fn validate(&self) -> anyhow::Result<()> {
        if self.imageUrl.is_empty() {
            anyhow::bail!("Image URL must not be empty");
        };
        Url::parse(&self.imageUrl).map(|_| ())?;

        if self.requirements.imageId == B256::default() {
            anyhow::bail!("Image ID must not be ZERO");
        };
        if self.offer.timeout == 0 {
            anyhow::bail!("Offer timeout must be greater than 0");
        };
        if self.offer.maxPrice == U96::ZERO {
            anyhow::bail!("Offer maxPrice must be greater than 0");
        };
        if self.offer.maxPrice < self.offer.minPrice {
            anyhow::bail!("Offer maxPrice must be greater than or equal to minPrice");
        }
        if self.offer.biddingStart == 0 {
            anyhow::bail!("Offer biddingStart must be greater than 0");
        };

        Ok(())
    }
}

#[cfg(not(target_os = "zkvm"))]
impl ProvingRequest {
    /// Signs the proving request with the given signer and EIP-712 domain derived from the given
    /// contract address and chain ID.
    pub fn sign_request(
        &self,
        signer: &impl SignerSync,
        contract_addr: Address,
        chain_id: u64,
    ) -> Result<Signature, SignerErr> {
        let domain = eip712_domain(contract_addr, chain_id);
        let hash = self.eip712_signing_hash(&domain.alloy_struct());
        signer.sign_hash_sync(&hash)
    }

    /// Verifies the proving request signature with the given signer and EIP-712 domain derived from
    /// the given contract address and chain ID.
    pub fn verify_signature(
        &self,
        signature: &Bytes,
        contract_addr: Address,
        chain_id: u64,
    ) -> Result<(), SignerErr> {
        let sig = Signature::try_from(signature.as_ref())?;
        let domain = eip712_domain(contract_addr, chain_id);
        let hash = self.eip712_signing_hash(&domain.alloy_struct());
        let addr = sig.recover_address_from_prehash(&hash)?;
        if addr == self.client_address() {
            Ok(())
        } else {
            Err(SignerErr::SignatureError(SignatureError::FromBytes("Address mismatch")))
        }
    }
}

impl Requirements {
    /// Creates a new requirements with the given image ID and predicate.
    pub fn new(image_id: impl Into<Digest>, predicate: Predicate) -> Self {
        Self { imageId: <[u8; 32]>::from(image_id.into()).into(), predicate }
    }

    /// Sets the image ID.
    pub fn with_image_id(self, image_id: impl Into<Digest>) -> Self {
        Self { imageId: <[u8; 32]>::from(image_id.into()).into(), ..self }
    }

    /// Sets the predicate.
    pub fn with_predicate(self, predicate: Predicate) -> Self {
        Self { predicate, ..self }
    }
}

impl Predicate {
    /// Returns a predicate to match the journal digest. This ensures that the proving request's
    /// fulfillment will contain a journal with the same digest.
    pub fn digest_match(digest: impl Into<Digest>) -> Self {
        Self {
            predicateType: PredicateType::DigestMatch,
            data: digest.into().as_bytes().to_vec().into(),
        }
    }

    /// Returns a predicate to match the journal prefix. This ensures that the proving request's
    /// fulfillment will contain a journal with the same prefix.
    pub fn prefix_match(prefix: impl Into<Bytes>) -> Self {
        Self { predicateType: PredicateType::PrefixMatch, data: prefix.into() }
    }
}

impl Input {
    /// Sets the input type to inline and the data to the given bytes.
    pub fn inline(data: impl Into<Bytes>) -> Self {
        Self { inputType: InputType::Inline, data: data.into() }
    }

    /// Sets the input type to URL and the data to the given URL.
    pub fn url(url: impl Into<String>) -> Self {
        Self { inputType: InputType::Url, data: url.into().into() }
    }
}

impl Offer {
    /// Sets the offer minimum price.
    pub fn with_min_price(self, min_price: U96) -> Self {
        Self { minPrice: min_price, ..self }
    }

    /// Sets the offer maximum price.
    pub fn with_max_price(self, max_price: U96) -> Self {
        Self { maxPrice: max_price, ..self }
    }

    /// Sets the offer lock-in stake.
    pub fn with_lockin_stake(self, lockin_stake: U96) -> Self {
        Self { lockinStake: lockin_stake, ..self }
    }

    /// Sets the offer bidding start as block number.
    pub fn with_bidding_start(self, bidding_start: u64) -> Self {
        Self { biddingStart: bidding_start, ..self }
    }

    /// Sets the offer timeout as number of blocks from the bidding start before expiring.
    pub fn with_timeout(self, timeout: u32) -> Self {
        Self { timeout, ..self }
    }

    /// Sets the offer ramp-up period as number of blocks from the bidding start before the price
    /// starts to increase until the maximum price.
    pub fn with_ramp_up_period(self, ramp_up_period: u32) -> Self {
        Self { rampUpPeriod: ramp_up_period, ..self }
    }

    /// Sets the offer minimum price based on the desired price per million cycles.
    pub fn with_min_price_per_mcycle(self, mcycle_price: U96, mcycle: u64) -> Self {
        let min_price = mcycle_price * U96::from(mcycle);
        Self { minPrice: min_price, ..self }
    }

    /// Sets the offer maximum price based on the desired price per million cycles.
    pub fn with_max_price_per_mcycle(self, mcycle_price: U96, mcycle: u64) -> Self {
        let max_price = mcycle_price * U96::from(mcycle);
        Self { maxPrice: max_price, ..self }
    }

    /// Sets the offer lock-in stake based on the desired price per million cycles.
    pub fn with_lockin_stake_per_mcycle(self, mcycle_price: U96, mcycle: u64) -> Self {
        let lockin_stake = mcycle_price * U96::from(mcycle);
        Self { lockinStake: lockin_stake, ..self }
    }
}

// TODO: These are not so much "default" as they are "empty". Default is not quite the right
// semantics here. This would be replaced by a builder or an `empty` function.
impl Default for ProvingRequest {
    fn default() -> Self {
        Self {
            id: U192::ZERO,
            requirements: Default::default(),
            imageUrl: Default::default(),
            input: Default::default(),
            offer: Default::default(),
        }
    }
}

impl Default for Requirements {
    fn default() -> Self {
        Self { imageId: Default::default(), predicate: Default::default() }
    }
}

impl Default for Predicate {
    fn default() -> Self {
        Self { predicateType: PredicateType::PrefixMatch, data: Default::default() }
    }
}

impl Default for Input {
    fn default() -> Self {
        Self { inputType: InputType::Inline, data: Default::default() }
    }
}

#[cfg(not(target_os = "zkvm"))]
alloy::sol!(
    #![sol(rpc, all_derives)]
    "../../contracts/src/IRiscZeroSetVerifier.sol"
);

use sha2::{Digest as _, Sha256};
#[cfg(not(target_os = "zkvm"))]
use IProofMarket::IProofMarketErrors;
#[cfg(not(target_os = "zkvm"))]
use IRiscZeroSetVerifier::IRiscZeroSetVerifierErrors;

impl Predicate {
    /// Evaluates the predicate against the given journal.
    #[inline]
    pub fn eval(&self, journal: impl AsRef<[u8]>) -> bool {
        match self.predicateType {
            PredicateType::DigestMatch => self.data.as_ref() == Sha256::digest(journal).as_slice(),
            PredicateType::PrefixMatch => journal.as_ref().starts_with(&self.data),
            PredicateType::__Invalid => panic!("invalid PredicateType"),
        }
    }
}

#[cfg(not(target_os = "zkvm"))]
pub mod proof_market;
#[cfg(not(target_os = "zkvm"))]
pub mod set_verifier;

#[cfg(not(target_os = "zkvm"))]
#[derive(Error, Debug)]
pub enum TxnErr {
    #[error("SetVerifier error: {0:?}")]
    SetVerifierErr(IRiscZeroSetVerifierErrors),

    #[error("ProofMarket Err: {0:?}")]
    ProofMarketErr(IProofMarket::IProofMarketErrors),

    #[error("decoding err, missing data, code: {0} msg: {1}")]
    MissingData(i64, String),

    #[error("decoding err: bytes decoding")]
    BytesDecode,

    #[error("contract error: {0}")]
    ContractErr(#[from] ContractErr),

    #[error("abi decoder error: {0} - {1}")]
    DecodeErr(DecoderErr, Bytes),
}

#[cfg(not(target_os = "zkvm"))]
fn decode_contract_err<T: SolInterface>(err: ContractErr) -> Result<T, TxnErr> {
    match err {
        ContractErr::TransportError(TransportError::ErrorResp(ts_err)) => {
            let Some(data) = ts_err.data else {
                return Err(TxnErr::MissingData(ts_err.code, ts_err.message));
            };

            let data = data.get().trim_matches('"');

            let Ok(data) = Bytes::from_str(data) else {
                return Err(TxnErr::BytesDecode);
            };

            let decoded_error = match T::abi_decode(&data, true) {
                Ok(res) => res,
                Err(err) => {
                    return Err(TxnErr::DecodeErr(err, data));
                }
            };

            Ok(decoded_error)
        }
        _ => Err(TxnErr::ContractErr(err)),
    }
}

#[cfg(not(target_os = "zkvm"))]
impl IRiscZeroSetVerifierErrors {
    pub fn decode_error(err: ContractErr) -> TxnErr {
        match decode_contract_err(err) {
            Ok(res) => TxnErr::SetVerifierErr(res),
            Err(decode_err) => decode_err,
        }
    }
}
#[cfg(not(target_os = "zkvm"))]
impl IProofMarketErrors {
    pub fn decode_error(err: ContractErr) -> TxnErr {
        match decode_contract_err(err) {
            Ok(res) => TxnErr::ProofMarketErr(res),
            Err(decode_err) => decode_err,
        }
    }
}

#[cfg(not(target_os = "zkvm"))]
pub fn eip712_domain(addr: Address, chain_id: u64) -> EIP721DomainSaltless {
    EIP721DomainSaltless {
        name: "IProofMarket".into(),
        version: "1".into(),
        chain_id,
        verifying_contract: addr,
    }
}

#[cfg(feature = "test-utils")]
pub mod test_utils {
    use aggregation_set::SET_BUILDER_GUEST_ID;
    use alloy::{
        network::{Ethereum, EthereumWallet},
        node_bindings::AnvilInstance,
        primitives::{Address, FixedBytes, U256},
        providers::{
            ext::AnvilApi,
            fillers::{
                BlobGasFiller, ChainIdFiller, FillProvider, GasFiller, JoinFill, NonceFiller,
                WalletFiller,
            },
            Identity, ProviderBuilder, RootProvider,
        },
        signers::local::PrivateKeySigner,
        transports::BoxTransport,
    };
    use anyhow::Result;
    use guest_assessor::ASSESSOR_GUEST_ID;
    use risc0_zkvm::sha::Digest;

    use crate::contracts::{proof_market::ProofMarketService, set_verifier::SetVerifierService};

    alloy::sol!(
        #![sol(rpc)]
        MockVerifier,
        "../../contracts/out/RiscZeroMockVerifier.sol/RiscZeroMockVerifier.json"
    );

    alloy::sol!(
        #![sol(rpc)]
        SetVerifier,
        "../../contracts/out/RiscZeroSetVerifier.sol/RiscZeroSetVerifier.json"
    );

    alloy::sol!(
        #![sol(rpc)]
        ProofMarket,
        "../../contracts/out/ProofMarket.sol/ProofMarket.json"
    );

    // Note: I was completely unable to solve this with generics or trait objects
    type ProviderWallet = FillProvider<
        JoinFill<
            JoinFill<
                Identity,
                JoinFill<GasFiller, JoinFill<BlobGasFiller, JoinFill<NonceFiller, ChainIdFiller>>>,
            >,
            WalletFiller<EthereumWallet>,
        >,
        RootProvider<BoxTransport>,
        BoxTransport,
        Ethereum,
    >;

    pub struct TestCtx {
        pub verifier_addr: Address,
        pub set_verifier_addr: Address,
        pub proof_market_addr: Address,
        pub prover_signer: PrivateKeySigner,
        pub customer_signer: PrivateKeySigner,
        pub prover_provider: ProviderWallet,
        pub prover_market: ProofMarketService<BoxTransport, ProviderWallet>,
        pub customer_provider: ProviderWallet,
        pub customer_market: ProofMarketService<BoxTransport, ProviderWallet>,
        pub set_verifier: SetVerifierService<BoxTransport, ProviderWallet>,
    }

    impl TestCtx {
        async fn deploy_contracts(anvil: &AnvilInstance) -> Result<(Address, Address, Address)> {
            let deployer_signer: PrivateKeySigner = anvil.keys()[0].clone().into();
            let deployer_provider = ProviderBuilder::new()
                .with_recommended_fillers()
                .wallet(EthereumWallet::from(deployer_signer.clone()))
                .on_builtin(&anvil.endpoint())
                .await
                .unwrap();

            let verifier =
                MockVerifier::deploy(&deployer_provider, FixedBytes::ZERO).await.unwrap();

            let set_verifier = SetVerifier::deploy(
                &deployer_provider,
                *verifier.address(),
                <[u8; 32]>::from(Digest::from(SET_BUILDER_GUEST_ID)).into(),
                String::new(),
            )
            .await
            .unwrap();

            let proof_market = ProofMarket::deploy(
                &deployer_provider,
                *set_verifier.address(),
                <[u8; 32]>::from(Digest::from(ASSESSOR_GUEST_ID)).into(),
                String::new(),
            )
            .await
            .unwrap();

            // Mine forward some blocks
            deployer_provider.anvil_mine(Some(U256::from(10)), Some(U256::from(2))).await.unwrap();
            deployer_provider.anvil_set_interval_mining(2).await.unwrap();

            Ok((*verifier.address(), *set_verifier.address(), *proof_market.address()))
        }

        pub async fn new(anvil: &AnvilInstance) -> Result<Self> {
            let (verifier_addr, set_verifier_addr, proof_market_addr) =
                TestCtx::deploy_contracts(&anvil).await.unwrap();

            let prover_signer: PrivateKeySigner = anvil.keys()[1].clone().into();
            let customer_signer: PrivateKeySigner = anvil.keys()[2].clone().into();
            let verifier_signer: PrivateKeySigner = anvil.keys()[0].clone().into();

            let prover_provider = ProviderBuilder::new()
                .with_recommended_fillers()
                .wallet(EthereumWallet::from(prover_signer.clone()))
                .on_builtin(&anvil.endpoint())
                .await
                .unwrap();
            let customer_provider = ProviderBuilder::new()
                .with_recommended_fillers()
                .wallet(EthereumWallet::from(customer_signer.clone()))
                .on_builtin(&anvil.endpoint())
                .await
                .unwrap();
            let verifier_provider = ProviderBuilder::new()
                .with_recommended_fillers()
                .wallet(EthereumWallet::from(verifier_signer.clone()))
                .on_builtin(&anvil.endpoint())
                .await
                .unwrap();

            let prover_market = ProofMarketService::new(
                proof_market_addr,
                prover_provider.clone(),
                prover_signer.address(),
            );

            let customer_market = ProofMarketService::new(
                proof_market_addr,
                customer_provider.clone(),
                customer_signer.address(),
            );

            let set_verifier = SetVerifierService::new(
                set_verifier_addr,
                verifier_provider,
                verifier_signer.address(),
            );

            Ok(TestCtx {
                verifier_addr,
                set_verifier_addr,
                proof_market_addr,
                prover_signer,
                customer_signer,
                prover_provider,
                prover_market,
                customer_provider,
                customer_market,
                set_verifier,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::signers::local::PrivateKeySigner;

    fn create_order(
        signer: &impl SignerSync,
        signer_addr: Address,
        order_id: u32,
        contract_addr: Address,
        chain_id: u64,
    ) -> (ProvingRequest, [u8; 65]) {
        let request_id = request_id(&signer_addr, order_id);

        let req = ProvingRequest {
            id: request_id,
            requirements: Requirements {
                imageId: B256::ZERO,
                predicate: Predicate {
                    predicateType: PredicateType::PrefixMatch,
                    data: Default::default(),
                },
            },
            imageUrl: "test".to_string(),
            input: Input { inputType: InputType::Url, data: Default::default() },
            offer: Offer {
                minPrice: U96::from(0),
                maxPrice: U96::from(1),
                biddingStart: 0,
                timeout: 1000,
                rampUpPeriod: 1,
                lockinStake: U96::from(0),
            },
        };

        let client_sig = req.sign_request(&signer, contract_addr, chain_id).unwrap();

        (req, client_sig.as_bytes())
    }

    #[test]
    fn validate_sig() {
        let signer: PrivateKeySigner =
            "6f142508b4eea641e33cb2a0161221105086a84584c74245ca463a49effea30b".parse().unwrap();
        let order_id: u32 = 1;
        let contract_addr = Address::ZERO;
        let chain_id = 1;
        let signer_addr = signer.address();

        let (req, client_sig) =
            create_order(&signer, signer_addr, order_id, contract_addr, chain_id);

        req.verify_signature(&Bytes::from(client_sig), contract_addr, chain_id).unwrap();
    }

    #[test]
    #[should_panic(expected = "SignatureError")]
    fn invalid_sig() {
        let signer: PrivateKeySigner =
            "6f142508b4eea641e33cb2a0161221105086a84584c74245ca463a49effea30b".parse().unwrap();
        let order_id: u32 = 1;
        let contract_addr = Address::ZERO;
        let chain_id = 1;
        let signer_addr = signer.address();

        let (req, mut client_sig) =
            create_order(&signer, signer_addr, order_id, contract_addr, chain_id);

        client_sig[0] = 1;
        req.verify_signature(&Bytes::from(client_sig), contract_addr, chain_id).unwrap();
    }
}
