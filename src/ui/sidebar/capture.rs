//! Deterministic text+style capture of the rendered sidebar.
//!
//! This is a measurement instrument, not a feature. It exists so that a sidebar
//! rendering can be compared against another *commit's* rendering of the same
//! fixture at the same width — which is the only honest way to judge a visual
//! change, since a test that asserts against itself cannot tell you whether the
//! result got better.
//!
//! It is a child module of `ui::sidebar` on purpose: that gives it access to the
//! private `render_workspace_list` without widening anything's visibility for
//! the sake of a test.
