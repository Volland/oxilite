//! The lean-elpi engine behind the `native` feature; without it every run is `Unsupported`.

use crate::error::Result;
use crate::host::Host;
use crate::run::{Options, Solved};

#[cfg(feature = "native")]
mod native {
    use super::*;
    use crate::error::ElpiError;
    use crate::host::SIGNATURES;
    use crate::run::PRELUDE;
    use crate::term::ETerm;
    use leanelpi::{Program, Term};
    use std::sync::{Arc, Mutex, OnceLock};

    /// Compiled programs kept for reuse: compiling dominates short runs.
    const CACHED: usize = 32;

    type Cache = Mutex<Vec<(String, Arc<Program>)>>;

    fn cache() -> &'static Cache {
        static C: OnceLock<Cache> = OnceLock::new();
        C.get_or_init(|| Mutex::new(Vec::new()))
    }

    /// The prelude, the program, and the host predicates declared.
    pub fn compile(program: &str) -> Result<Arc<Program>> {
        let text = format!("{PRELUDE}\n{program}");
        let c = cache();
        if let Some(p) = c
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .iter()
            .find(|(t, _)| *t == text)
            .map(|(_, p)| p.clone())
        {
            return Ok(p);
        }
        let program_err = |e: leanelpi::Error| ElpiError::Program(e.to_string());
        let mut p = Program::new(&text).map_err(program_err)?;
        for (name, sig) in SIGNATURES {
            p = p.declare(name, sig).map_err(program_err)?;
        }
        let p = Arc::new(p);
        let mut g = c.lock().unwrap_or_else(|p| p.into_inner());
        if g.len() >= CACHED {
            g.remove(0);
        }
        g.push((text, p.clone()));
        Ok(p)
    }

    fn to_e(t: &Term) -> ETerm {
        match t {
            Term::Const(c) => ETerm::Const(c.clone()),
            Term::App(c, a) => ETerm::App(c.clone(), a.iter().map(to_e).collect()),
            Term::Int(n) => ETerm::Int(*n),
            Term::Float(x) => ETerm::Float(*x),
            Term::Str(s) => ETerm::Str(s.clone()),
            Term::Var(v) => ETerm::Var(v.clone()),
            Term::Other(o) => ETerm::Other(o.clone()),
        }
    }

    fn from_e(t: &ETerm) -> Term {
        match t {
            ETerm::Const(c) => Term::Const(c.clone()),
            ETerm::App(c, a) => Term::App(c.clone(), a.iter().map(from_e).collect()),
            ETerm::Int(n) => Term::Int(*n),
            ETerm::Float(x) => Term::Float(*x),
            ETerm::Str(s) => Term::Str(s.clone()),
            ETerm::Var(v) => Term::Var(v.clone()),
            ETerm::Other(o) => Term::Other(o.clone()),
        }
    }

    pub fn solve(
        program: &str,
        goal: &str,
        options: &Options,
        host: &mut Host<'_>,
    ) -> Result<Solved> {
        let p = compile(program)?;
        let run = p.solve_host(
            goal,
            options.limit,
            options.fuel,
            |name: &str, args: &[Term]| {
                let args: Vec<ETerm> = args.iter().map(to_e).collect();
                host.call(name, &args).map(|rows| {
                    rows.iter()
                        .map(|r| r.iter().map(from_e).collect())
                        .collect()
                })
            },
        );
        let run = match run {
            Ok(r) => r,
            Err(e) if host.over_budget => return Err(ElpiError::Budget(e.to_string())),
            Err(e) if e.is_out_of_fuel() => return Err(ElpiError::OutOfFuel(e.to_string())),
            Err(e) => return Err(ElpiError::Run(e.to_string())),
        };
        Ok(Solved {
            solutions: run
                .solutions
                .into_iter()
                .map(|s| {
                    (
                        s.terms.iter().map(|(k, v)| (k.clone(), to_e(v))).collect(),
                        s.bindings,
                        s.constraints,
                    )
                })
                .collect(),
            exhausted: run.exhausted,
            fuel_used: run.fuel_used,
        })
    }

    pub fn version() -> Option<String> {
        leanelpi::version().ok()
    }
}

/// Solves `goal` with the native engine.
#[cfg(feature = "native")]
pub(crate) fn solve(
    program: &str,
    goal: &str,
    options: &Options,
    host: &mut Host<'_>,
) -> Result<Solved> {
    native::solve(program, goal, options, host)
}

#[cfg(not(feature = "native"))]
pub(crate) fn solve(_: &str, _: &str, _: &Options, _: &mut Host<'_>) -> Result<Solved> {
    Err(crate::run::unsupported())
}

/// The lean-elpi version, when the engine is linked.
pub fn version() -> Option<String> {
    #[cfg(feature = "native")]
    return native::version();
    #[cfg(not(feature = "native"))]
    None
}

/// Whether this build carries the native engine.
pub const fn available() -> bool {
    cfg!(feature = "native")
}
