use ody_models_manager::collaboration_mode_presets::collaboration_mode_presets_for;
use ody_protocol::config_types::CollaborationModeMask;
use ody_protocol::config_types::ModeKind;

use crate::model_catalog::ModelCatalog;

/// Presets this invocation may offer, in ring order.
///
/// Opt-in modes (`Game`, `Frontend`) only appear when the catalog was built with
/// them enabled, so a plain `ody` never shows a mode the user cannot enter —
/// and `shift+tab` never lands on one either.
fn filtered_presets(model_catalog: &ModelCatalog) -> Vec<CollaborationModeMask> {
    collaboration_mode_presets_for(model_catalog.mode_availability())
}

pub(crate) fn default_mask(model_catalog: &ModelCatalog) -> Option<CollaborationModeMask> {
    let presets = filtered_presets(model_catalog);
    presets
        .iter()
        .find(|mask| mask.mode == Some(ModeKind::Default))
        .cloned()
        .or_else(|| presets.into_iter().next())
}

pub(crate) fn mask_for_kind(
    model_catalog: &ModelCatalog,
    kind: ModeKind,
) -> Option<CollaborationModeMask> {
    if !model_catalog.mode_availability().is_available(kind) {
        return None;
    }
    filtered_presets(model_catalog)
        .into_iter()
        .find(|mask| mask.mode == Some(kind))
}

/// Cycle to the next collaboration mode preset in list order.
pub(crate) fn next_mask(
    model_catalog: &ModelCatalog,
    current: Option<&CollaborationModeMask>,
) -> Option<CollaborationModeMask> {
    let presets = filtered_presets(model_catalog);
    if presets.is_empty() {
        return None;
    }
    let current_kind = current.and_then(|mask| mask.mode);
    let next_index = presets
        .iter()
        .position(|mask| mask.mode == current_kind)
        .map_or(0, |idx| (idx + 1) % presets.len());
    presets.get(next_index).cloned()
}

pub(crate) fn default_mode_mask(model_catalog: &ModelCatalog) -> Option<CollaborationModeMask> {
    mask_for_kind(model_catalog, ModeKind::Default)
}

pub(crate) fn plan_mask(model_catalog: &ModelCatalog) -> Option<CollaborationModeMask> {
    mask_for_kind(model_catalog, ModeKind::Plan)
}

pub(crate) fn design_mask(model_catalog: &ModelCatalog) -> Option<CollaborationModeMask> {
    mask_for_kind(model_catalog, ModeKind::Design)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ody_protocol::config_types::ModeAvailability;

    fn catalog(availability: ModeAvailability) -> ModelCatalog {
        ModelCatalog::new(Vec::new()).with_mode_availability(availability)
    }

    #[test]
    fn plain_catalog_never_offers_opt_in_modes() {
        let catalog = catalog(ModeAvailability::default());
        for kind in [ModeKind::Game, ModeKind::Frontend] {
            assert!(
                mask_for_kind(&catalog, kind).is_none(),
                "{kind:?} must not be selectable without an opt-in"
            );
        }
        // The ring is exactly the always-on four, starting at Default.
        let mut seen = Vec::new();
        let mut current = default_mask(&catalog);
        for _ in 0..5 {
            let kind = current.as_ref().and_then(|mask| mask.mode).expect("mode");
            seen.push(kind);
            current = next_mask(&catalog, current.as_ref());
        }
        assert_eq!(
            seen,
            vec![
                ModeKind::Default,
                ModeKind::Plan,
                ModeKind::Design,
                ModeKind::Product,
                ModeKind::Default,
            ]
        );
    }

    #[test]
    fn opt_in_modes_join_the_ring_only_when_enabled() {
        let catalog = catalog(ModeAvailability::new(
            /*game*/ false, /*frontend*/ true,
        ));
        assert!(mask_for_kind(&catalog, ModeKind::Frontend).is_some());
        assert!(mask_for_kind(&catalog, ModeKind::Game).is_none());

        // The opt-in mode sits directly after Default in preset order, so the
        // first `shift+tab` out of Default lands on it.
        let mut current = mask_for_kind(&catalog, ModeKind::Default);
        let mut seen = Vec::new();
        for _ in 0..5 {
            current = next_mask(&catalog, current.as_ref());
            seen.push(current.as_ref().and_then(|mask| mask.mode).expect("mode"));
        }
        assert_eq!(
            seen,
            vec![
                ModeKind::Frontend,
                ModeKind::Plan,
                ModeKind::Design,
                ModeKind::Product,
                ModeKind::Default,
            ]
        );
    }

    #[test]
    fn opt_in_preset_carries_the_mode_contract() {
        let catalog = catalog(ModeAvailability::new(
            /*game*/ true, /*frontend*/ true,
        ));
        for kind in [ModeKind::Game, ModeKind::Frontend] {
            let mask = mask_for_kind(&catalog, kind).expect("mode must be offered");
            let instructions = mask
                .developer_instructions
                .flatten()
                .unwrap_or_else(|| panic!("{kind:?} preset must carry developer instructions"));
            assert!(
                instructions.contains("`SKILL.md`"),
                "{kind:?} preset must send the model to its bundle"
            );
        }
    }
}
