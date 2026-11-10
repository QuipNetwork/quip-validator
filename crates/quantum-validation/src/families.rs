//! Canonical family regeneration and witness checks.
//!
//! The validator never trusts miner-supplied coefficients. It rebuilds each
//! family instance from three inputs that it already holds:
//!
//! - the nonce, from [`crate::derive_nonce`]
//! - the family descriptor: family id, revision, and canonical parameter bytes
//! - the base topology, in registration order
//!
//! All generation code comes from the canonical `ising-generators` crate, so
//! the bytes match the solver-core golden vectors exactly.
//!
//! This module only regenerates and checks. It does not decide which family a
//! challenge uses. Every revision in [`CONTRACTS`] has registry status
//! `research`, and no caller in the runtime reaches this module yet.

use ising_generators::descriptor::{self, params_digest, Admitted, SizeBounds};
use ising_generators::family::{self, BaseTopology, SaltFault};

pub use ising_generators::descriptor::{AdvertisedLimits, Contract};
pub use ising_generators::family::Defect;
pub use ising_generators::instance::Instance;
pub use ising_generators::Code;

use crate::fixed::MilliEnergy;

/// Every family revision that the validator can regenerate.
///
/// The list is the set of canonical constructions in `ising-generators`.
/// Inclusion here does not authorize a revision for proof of work.
pub const CONTRACTS: &[Contract] = &[
    ising_generators::ice::glass::ICE_GLASS_V1,
    ising_generators::ice::twisted_dimer::TWISTED_DIMER_V1,
    ising_generators::ice::charge::CHARGED_ICE_V1,
    ising_generators::ice::fields::ICE_FIELDS_V1,
    ising_generators::routing::ROUTING_V1,
    ising_generators::polariton::POLARITON_V1,
    ising_generators::defects::DEFECTS_V1,
    ising_generators::flow::FLOW_V1,
];

/// Why a family challenge, instance, or witness is refused.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FamilyError {
    /// Admission refused the descriptor or the topology, with its stable code.
    Rejected(Code),
    /// The topology has a duplicate node, an unknown edge endpoint, or a count
    /// above `u32::MAX`.
    InvalidTopology,
    /// The nonce gives no instance: code 13 (a bound) or 14 (no instance).
    Skipped(Code),
    /// The canonical construction broke its contract for this nonce.
    GeneratorDefect(Defect),
    /// The instance was not built for this challenge.
    ForeignInstance,
    /// The witness has the wrong number of spins.
    WitnessLength { expected: usize, actual: usize },
    /// A witness entry is not `-1` or `+1`.
    InvalidSpin { index: usize, value: i8 },
    /// The witness breaks a hard constraint of the family.
    Infeasible,
    /// The energy does not fit in `i64`.
    ArithmeticOverflow,
}

/// A family descriptor and base topology that passed admission.
///
/// Admission runs once per challenge. [`Self::regenerate`] then runs once per
/// nonce, and [`Self::verify_witness`] once per submitted witness.
///
/// Work is bounded at admission. The contract limits and `limits` cap the
/// node and edge counts of every instance, so generation and energy
/// evaluation are linear in sizes that admission has already accepted.
#[derive(Clone, Debug)]
pub struct FamilyChallenge<'a> {
    admitted: Admitted<'a>,
    topology: BaseTopology,
    bounds: SizeBounds,
}

impl<'a> FamilyChallenge<'a> {
    /// Admit a descriptor against [`CONTRACTS`] and build its base topology.
    ///
    /// `nodes` and `edges` are native ids in registration order. The order is
    /// part of the challenge: the topology order digest covers it.
    ///
    /// # Errors
    ///
    /// [`FamilyError::Rejected`] with the admission code, or
    /// [`FamilyError::InvalidTopology`].
    pub fn admit(
        family_id: &'a [u8],
        revision: u32,
        params: &'a [u8],
        nodes: &[u32],
        edges: &[(u32, u32)],
        limits: &AdvertisedLimits,
    ) -> Result<Self, FamilyError> {
        // The validator holds the parameter bytes itself, so the digest it
        // checks is its own.
        let admitted = descriptor::admit(
            CONTRACTS,
            family_id,
            revision,
            params,
            &params_digest(params),
        )
        .map_err(FamilyError::Rejected)?;
        let topology =
            BaseTopology::new(nodes.to_vec(), edges).map_err(|_| FamilyError::InvalidTopology)?;
        let bounds = family::admit_topology(&admitted, &topology, topology.order_digest(), limits)
            .map_err(FamilyError::Rejected)?;
        Ok(Self {
            admitted,
            topology,
            bounds,
        })
    }

    /// The descriptor digest of the admitted descriptor.
    pub fn descriptor_digest(&self) -> [u8; 32] {
        self.admitted.descriptor().digest()
    }

    /// The topology order digest of the base topology.
    pub fn topology_order_digest(&self) -> [u8; 32] {
        *self.topology.order_digest()
    }

    /// The size bounds that admission computed for every instance.
    pub fn size_bounds(&self) -> SizeBounds {
        self.bounds
    }

    /// Rebuild the canonical instance of `nonce`.
    ///
    /// The canonical generator also checks the instance layout, the size
    /// bounds, and the family metadata.
    ///
    /// # Errors
    ///
    /// [`FamilyError::Skipped`] when the nonce gives no instance, or
    /// [`FamilyError::GeneratorDefect`].
    pub fn regenerate(&self, nonce: [u8; 32]) -> Result<Instance, FamilyError> {
        family::generate_instance(&self.admitted, &self.topology, &self.bounds, nonce).map_err(
            |fault| match fault {
                SaltFault::Skip(code) => FamilyError::Skipped(code),
                SaltFault::Defect(defect) => FamilyError::GeneratorDefect(defect),
            },
        )
    }

    /// Check `spins` against `instance` and return its exact energy.
    ///
    /// `spins` has one entry per instance node, in instance node order. The
    /// checks run in this order: the instance belongs to this challenge, the
    /// witness length, the spin alphabet, and the hard constraints of the
    /// family.
    ///
    /// # Errors
    ///
    /// [`FamilyError::ForeignInstance`], [`FamilyError::WitnessLength`],
    /// [`FamilyError::InvalidSpin`], [`FamilyError::Infeasible`], or
    /// [`FamilyError::ArithmeticOverflow`].
    pub fn verify_witness(
        &self,
        instance: &Instance,
        spins: &[i8],
    ) -> Result<MilliEnergy, FamilyError> {
        if *instance.descriptor_digest() != self.descriptor_digest()
            || instance.topology_order_digest() != self.topology.order_digest()
        {
            return Err(FamilyError::ForeignInstance);
        }
        check_spins(instance, spins)?;
        if !family::feasible(&self.admitted, instance, spins) {
            return Err(FamilyError::Infeasible);
        }
        instance_energy(instance, spins)
    }
}

/// The witness length and the `{-1, +1}` alphabet.
fn check_spins(instance: &Instance, spins: &[i8]) -> Result<(), FamilyError> {
    let expected = instance.node_ids().len();
    if spins.len() != expected {
        return Err(FamilyError::WitnessLength {
            expected,
            actual: spins.len(),
        });
    }
    match spins.iter().position(|&s| s != 1 && s != -1) {
        Some(index) => Err(FamilyError::InvalidSpin {
            index,
            value: spins[index],
        }),
        None => Ok(()),
    }
}

/// The exact energy of `spins` on `instance`, in milli, offset included.
///
/// The energy is `offset + sum(h_i s_i) + sum(J_uv s_u s_v)` with checked
/// `i64` accumulation. This function does not check hard constraints; use
/// [`FamilyChallenge::verify_witness`] for a proof.
///
/// # Errors
///
/// [`FamilyError::WitnessLength`], [`FamilyError::InvalidSpin`], or
/// [`FamilyError::ArithmeticOverflow`].
pub fn instance_energy(instance: &Instance, spins: &[i8]) -> Result<MilliEnergy, FamilyError> {
    check_spins(instance, spins)?;
    let overflow = FamilyError::ArithmeticOverflow;
    let mut energy = instance.offset_milli();
    for (&field, &spin) in instance.h().iter().zip(spins) {
        energy = energy
            .checked_add(i64::from(field) * i64::from(spin))
            .ok_or(overflow)?;
    }
    for (&(u, v), &coupling) in instance.edges().iter().zip(instance.j()) {
        // `Instance::new` guarantees every edge position is below the node
        // count, and `check_spins` fixed the witness length to that count.
        let spin_at = |position: u32| {
            usize::try_from(position)
                .ok()
                .and_then(|p| spins.get(p))
                .map(|&s| i64::from(s))
                .ok_or(overflow)
        };
        energy = energy
            .checked_add(i64::from(coupling) * spin_at(u)? * spin_at(v)?)
            .ok_or(overflow)?;
    }
    Ok(energy)
}
