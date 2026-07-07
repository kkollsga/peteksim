from __future__ import annotations

import pytest

import peteksim as ps

petekstatic = pytest.importorskip("petekstatic")


class _LogSource:
    def as_dict(self):
        return {"kind": "log_channel", "mnemonic": "PHIE"}


def test_static_workflow_names_delegate_to_petekstatic():
    for name in (
        "CoKriging",
        "DistributionSpec",
        "Grid",
        "PropertyPipeline",
        "PropertyPipelineSpec",
        "SgsRecipe",
        "Var",
        "WellLog",
        "WellLogSpec",
        "distributions",
        "upscale",
    ):
        assert getattr(ps, name) is getattr(petekstatic, name)


def test_canonical_property_recipe_reaches_petekstatic_pipeline_spec():
    recipe = ps.upscale(_LogSource()).sgs(
        variogram=ps.Var(
            "spherical",
            major=800.0,
            minor=800.0,
            vertical=40.0,
            azimuth=0.0,
        ),
        distribution=ps.distributions.from_logs(),
        seed=12,
    )

    spec = recipe.lower("PORO")

    assert isinstance(recipe, petekstatic.SgsRecipe)
    assert isinstance(spec, petekstatic.PropertyPipelineSpec)
    assert spec.property == "PORO"
    assert spec.distribution.kind == "from_logs"
    assert spec.seed == 12


def test_legacy_prop_specs_warn_toward_petekstatic_workflow():
    with pytest.warns(
        DeprecationWarning,
        match="canonical property workflow now lives in petekStatic",
    ):
        prop = ps.Prop("PORO", net_only=True)
    with pytest.warns(DeprecationWarning, match="peteksim.upscale"):
        props = ps.Props(prop)

    assert prop.name == "PORO"
    assert prop.net_only is True
    assert props.items == (prop,)
