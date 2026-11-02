//! Launch-only inventory. This validates an explicit custody plan; it neither
//! schedules transfers nor gives recipients an on-chain claim on custody.
use super::{mainnet_placeholder, reserved_mainnet_accounts};
use crate::{AccountId, Balance, BlockNumber, Runtime, EXISTENTIAL_DEPOSIT};
use alloc::{
    collections::{BTreeMap, BTreeSet},
    format,
    string::String,
    vec::Vec,
};

pub const MAINNET_ALLOCATION_TOTAL: Option<Balance> = None;
pub const MAINNET_ALLOCATION_RECIPIENTS: Option<&[AccountId]> = None;

#[derive(Clone)]
pub struct AllocationTranche {
    pub recipient: AccountId,
    pub issue_at: BlockNumber,
    pub schedule: pallet_vesting::VestingInfo<Balance, BlockNumber>,
}

#[derive(Clone)]
pub struct AllocationTerms {
    /// Explicit launch selection, not the provisional quarterly default.
    pub cadence_blocks: BlockNumber,
    /// Ordered by issue block within each recipient. Amounts, cliff start and
    /// rate are explicit so no rounding or global/per-grant cliff is invented.
    pub tranches: Vec<AllocationTranche>,
}

#[derive(Clone)]
pub(super) struct MainnetAllocation {
    pub total: Option<Balance>,
    pub custody: AccountId,
    pub recipients: Option<Vec<AccountId>>,
    pub terms: Option<AllocationTerms>,
}

pub(super) fn custody_placeholder() -> AccountId {
    mainnet_placeholder("custody", "Allocation")
}

impl MainnetAllocation {
    pub(super) fn unresolved() -> Self {
        Self {
            total: MAINNET_ALLOCATION_TOTAL,
            custody: custody_placeholder(),
            recipients: MAINNET_ALLOCATION_RECIPIENTS.map(<[AccountId]>::to_vec),
            terms: None, // TBD: explicitly reviewed tranche sizes, cadence and curves.
        }
    }

    pub(super) fn validate(&self, operational: &[AccountId]) -> Result<Balance, String> {
        let fail = |reason: &str| format!("mainnet genesis blocked: {reason}");
        let total = self
            .total
            .ok_or_else(|| fail("MAINNET_ALLOCATION_TOTAL is TBD"))?;
        if total < EXISTENTIAL_DEPOSIT {
            return Err(fail(
                "MAINNET_ALLOCATION_TOTAL must fund a usable custody account",
            ));
        }
        let reserved = reserved_mainnet_accounts();
        let revive = pallet_revive::Pallet::<Runtime>::account_id();
        if reserved.contains(&self.custody) {
            return Err(fail("unresolved mainnet custody placeholder"));
        }
        if operational.contains(&self.custody) || self.custody == revive {
            return Err(fail("mainnet custody must be a dedicated account"));
        }
        let recipients = self
            .recipients
            .as_ref()
            .ok_or_else(|| fail("MAINNET_ALLOCATION_RECIPIENTS is TBD"))?;
        let unique: BTreeSet<_> = recipients.iter().collect();
        if recipients.is_empty()
            || unique.len() != recipients.len()
            || recipients
                .iter()
                .any(|r| reserved.contains(r) || r == &self.custody || r == &revive)
        {
            return Err(fail("MAINNET_ALLOCATION_RECIPIENTS must be distinct resolved accounts separate from custody and Revive"));
        }
        // Recipient overlap with operational accounts, including validator
        // stashes, is explicitly allowed. Their existing balances are retained.
        let terms = self
            .terms
            .as_ref()
            .ok_or_else(|| fail("MAINNET_ALLOCATION_TERMS is TBD"))?;
        if terms.cadence_blocks == 0 || terms.tranches.is_empty() {
            return Err(fail(
                "allocation terms need a positive cadence and explicit tranches",
            ));
        }
        let mut issued: BTreeMap<&AccountId, (BlockNumber, u32)> = BTreeMap::new();
        let mut planned = 0u128;
        for tranche in &terms.tranches {
            let schedule = tranche.schedule;
            if !unique.contains(&tranche.recipient) {
                return Err(fail("allocation tranche names an unlisted recipient"));
            }
            if tranche.issue_at == 0
                || schedule.starting_block() < tranche.issue_at
                || !schedule.is_valid()
                || schedule.locked() < <Runtime as pallet_vesting::Config>::MinVestedTransfer::get()
            {
                return Err(fail(
                    "allocation tranche has unusable amount, rate or start",
                ));
            }
            // SDK duration is ceil(locked / rate). Check it without overflowing
            // and reject a saturated end block rather than silently accepting it.
            let duration = schedule
                .locked()
                .checked_sub(1)
                .and_then(|n| n.checked_div(schedule.per_block()))
                .and_then(|n| n.checked_add(1))
                .and_then(|n| BlockNumber::try_from(n).ok())
                .ok_or_else(|| fail("allocation tranche end block overflows"))?;
            schedule
                .starting_block()
                .checked_add(duration)
                .ok_or_else(|| fail("allocation tranche end block overflows"))?;
            let count = if let Some((previous, count)) = issued.get(&tranche.recipient) {
                if previous.checked_add(terms.cadence_blocks) != Some(tranche.issue_at) {
                    return Err(fail("allocation tranche cadence is inconsistent"));
                }
                count
                    .checked_add(1)
                    .ok_or_else(|| fail("allocation schedule count overflows"))?
            } else {
                1
            };
            if count > <Runtime as pallet_vesting::Config>::MAX_VESTING_SCHEDULES {
                return Err(fail(
                    "allocation plan exceeds the per-recipient schedule cap",
                ));
            }
            issued.insert(&tranche.recipient, (tranche.issue_at, count));
            planned = planned
                .checked_add(schedule.locked())
                .ok_or_else(|| fail("allocation tranche total overflows"))?;
        }
        if issued.len() != unique.len() {
            return Err(fail("allocation plan omits a recipient"));
        }
        let residual = total
            .checked_sub(planned)
            .ok_or_else(|| fail("allocation tranches exceed custody funding"))?;
        if residual != 0 && residual < EXISTENTIAL_DEPOSIT {
            return Err(fail("allocation plan leaves a dust custody residual"));
        }
        Ok(total)
    }
}

#[cfg(test)]
pub(super) fn fixture() -> MainnetAllocation {
    let recipient = AccountId::new([92; 32]);
    MainnetAllocation {
        total: Some(100 * crate::UNIT),
        custody: AccountId::new([91; 32]),
        recipients: Some(alloc::vec![recipient.clone()]),
        terms: Some(AllocationTerms {
            cadence_blocks: 10,
            tranches: alloc::vec![AllocationTranche {
                recipient,
                issue_at: 1,
                schedule: pallet_vesting::VestingInfo::new(20 * crate::UNIT, crate::UNIT, 10)
            }],
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::UNIT;
    type Schedule = pallet_vesting::VestingInfo<Balance, BlockNumber>;
    fn rejected(input: MainnetAllocation, expected: &str) {
        assert_eq!(
            input.validate(&[]).err().as_deref(),
            Some(format!("mainnet genesis blocked: {expected}").as_str())
        );
    }
    #[test]
    fn unresolved_allocation_fields_each_block_readiness() {
        assert!(MAINNET_ALLOCATION_TOTAL.is_none());
        assert!(MAINNET_ALLOCATION_RECIPIENTS.is_none());
        assert!(MainnetAllocation::unresolved().terms.is_none());
        let mut x = fixture();
        x.total = None;
        rejected(x, "MAINNET_ALLOCATION_TOTAL is TBD");
        let mut x = fixture();
        x.custody = custody_placeholder();
        rejected(x, "unresolved mainnet custody placeholder");
        let mut x = fixture();
        x.recipients = None;
        rejected(x, "MAINNET_ALLOCATION_RECIPIENTS is TBD");
        let mut x = fixture();
        x.terms = None;
        rejected(x, "MAINNET_ALLOCATION_TERMS is TBD");
    }
    #[test]
    fn amount_and_account_guards_preserve_recipient_staking_permission() {
        let x = fixture();
        assert_eq!(x.validate(x.recipients.as_ref().unwrap()), Ok(100 * UNIT));
        assert!(x.validate(core::slice::from_ref(&x.custody)).is_err());
        for amount in [0, EXISTENTIAL_DEPOSIT - 1] {
            let mut x = fixture();
            x.total = Some(amount);
            rejected(
                x,
                "MAINNET_ALLOCATION_TOTAL must fund a usable custody account",
            );
        }
        for recipients in [
            Vec::new(),
            alloc::vec![AccountId::new([92;32]);2],
            alloc::vec![custody_placeholder()],
            alloc::vec![fixture().custody],
            alloc::vec![pallet_revive::Pallet::<Runtime>::account_id()],
        ] {
            let mut x = fixture();
            x.recipients = Some(recipients);
            rejected(x,"MAINNET_ALLOCATION_RECIPIENTS must be distinct resolved accounts separate from custody and Revive");
        }
        let mut x = fixture();
        x.custody = pallet_revive::Pallet::<Runtime>::account_id();
        rejected(x, "mainnet custody must be a dedicated account");
    }
    #[test]
    fn tranche_plan_rejects_invalid_amounts_timing_inventory_and_funding() {
        for case in 0..12 {
            let mut x = fixture();
            let terms = x.terms.as_mut().unwrap();
            let expected = match case {
                0 => {
                    terms.cadence_blocks = 0;
                    "allocation terms need a positive cadence and explicit tranches"
                }
                1 => {
                    terms.tranches.clear();
                    "allocation terms need a positive cadence and explicit tranches"
                }
                2 => {
                    terms.tranches[0].recipient = AccountId::new([93; 32]);
                    "allocation tranche names an unlisted recipient"
                }
                3 => {
                    terms.tranches[0].schedule = Schedule::new(UNIT, 0, 10);
                    "allocation tranche has unusable amount, rate or start"
                }
                4 => {
                    terms.tranches[0].schedule = Schedule::new(EXISTENTIAL_DEPOSIT - 1, 1, 10);
                    "allocation tranche has unusable amount, rate or start"
                }
                5 => {
                    terms.tranches[0].issue_at = 0;
                    "allocation tranche has unusable amount, rate or start"
                }
                6 => {
                    terms.tranches[0].issue_at = 11;
                    "allocation tranche has unusable amount, rate or start"
                }
                7 => {
                    terms.tranches[0].schedule = Schedule::new(UNIT, 1, 10);
                    "allocation tranche end block overflows"
                }
                8 => {
                    terms.tranches[0].schedule = Schedule::new(UNIT, UNIT, BlockNumber::MAX);
                    "allocation tranche end block overflows"
                }
                9 => {
                    x.recipients
                        .as_mut()
                        .unwrap()
                        .push(AccountId::new([93; 32]));
                    "allocation plan omits a recipient"
                }
                10 => {
                    x.total = Some(10 * UNIT);
                    "allocation tranches exceed custody funding"
                }
                _ => {
                    x.total = Some(20 * UNIT + EXISTENTIAL_DEPOSIT - 1);
                    "allocation plan leaves a dust custody residual"
                }
            };
            rejected(x, expected);
        }
        let mut x = fixture();
        let terms = x.terms.as_mut().unwrap();
        let mut second = terms.tranches[0].clone();
        second.issue_at = 12;
        second.schedule = Schedule::new(UNIT, UNIT, 20);
        terms.tranches.push(second);
        rejected(x, "allocation tranche cadence is inconsistent");
        let mut x = fixture();
        x.total = Some(Balance::MAX);
        let terms = x.terms.as_mut().unwrap();
        terms.tranches[0].schedule = Schedule::new(Balance::MAX, Balance::MAX, 10);
        let mut second = terms.tranches[0].clone();
        second.issue_at = 11;
        second.schedule = Schedule::new(UNIT, UNIT, 20);
        terms.tranches.push(second);
        rejected(x, "allocation tranche total overflows");
    }
    #[test]
    fn tranche_floor_matches_the_runtime_transfer_floor() {
        let minimum = <Runtime as pallet_vesting::Config>::MinVestedTransfer::get();
        let mut x = fixture();
        x.terms.as_mut().unwrap().tranches[0].schedule =
            Schedule::new(minimum - 1, minimum - 1, 10);
        rejected(x, "allocation tranche has unusable amount, rate or start");
        let mut x = fixture();
        x.terms.as_mut().unwrap().tranches[0].schedule = Schedule::new(minimum, minimum, 10);
        assert!(x.validate(&[]).is_ok());
    }

    #[test]
    fn plan_respects_28_schedule_limit_without_changing_it_for_cadence() {
        let mut x = fixture();
        let terms = x.terms.as_mut().unwrap();
        let first = terms.tranches[0].clone();
        terms.tranches = (0..28)
            .map(|i| AllocationTranche {
                recipient: first.recipient.clone(),
                issue_at: 1 + i * 10,
                schedule: Schedule::new(UNIT, UNIT, 10 + i * 10),
            })
            .collect();
        assert!(x.validate(&[]).is_ok());
        x.terms.as_mut().unwrap().tranches.push(AllocationTranche {
            recipient: first.recipient,
            issue_at: 281,
            schedule: Schedule::new(UNIT, UNIT, 290),
        });
        rejected(x, "allocation plan exceeds the per-recipient schedule cap");
    }
}
