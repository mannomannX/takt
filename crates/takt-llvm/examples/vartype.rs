//! Zeigt, wie eine Blockinstanz getypt ist.
fn main() {
    let src = std::fs::read_to_string("corpus-try/18_blocks.takt").expect("lesbar");
    let o = takt_sema::Options { policy: takt_diag::Policy::default(), build: takt_sema::Build::Sim, profile: None };
    let out = takt_sema::compile(&src, &o);
    let Some(p) = out.program else { return };
    for b in &p.blocks {
        println!(
            "block {}: params={:?} state={:?}",
            b.name,
            b.params.iter().map(|x| &x.name).collect::<Vec<_>>(),
            b.state_vars.iter().map(|x| &x.name).collect::<Vec<_>>()
        );
        if let Some(fid) = b.step {
            let f = &p.fns[fid.index()];
            println!("  step locals: {:?}", f.locals.iter().map(|x| &x.name).collect::<Vec<_>>());
            println!("  step params: {:?}", f.params.iter().map(|x| &x.name).collect::<Vec<_>>());
        }
    }
    for m in &p.machines {
        println!("block_instances: {:?}", m.layout.block_instances);
        for v in &m.vars {
            println!("{} : {:?}", v.name, p.types.list.get(v.ty.index()));
            if let Some(takt_mir::types::Type::Optional(inner)) = p.types.list.get(v.ty.index()) {
                println!("   inner: {:?}", p.types.list.get(inner.index()));
            }
            println!("   init: {:?}", v.init.as_ref().map(|e| &e.kind));
        }
    }
}
