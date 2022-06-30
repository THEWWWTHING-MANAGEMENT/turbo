use std::{any::type_name, collections::HashMap};

use swc_common::{Span, Spanned};
use swc_ecmascript::{
    ast::*,
    visit::{noop_visit_mut_type, noop_visit_type, Visit, VisitMut, VisitMutWith, VisitWith},
};

pub type AstPath = Vec<Span>;

pub type BoxedVisitor = Box<dyn VisitMut + Send + Sync>;
pub type VisitorFn = Box<dyn Send + Sync + Fn() -> BoxedVisitor>;

pub struct ApplyVisitors<'a> {
    /// `VisitMut` should be shallow. In other words, it should not visit
    /// children of the node.
    visitors: HashMap<Span, Vec<(&'a AstPath, &'a VisitorFn)>>,

    index: usize,
}

impl<'a> ApplyVisitors<'a> {
    pub fn new(visitors: HashMap<Span, Vec<(&'a AstPath, &'a VisitorFn)>>) -> Self {
        Self { visitors, index: 0 }
    }

    fn visit_if_required<N>(&mut self, n: &mut N)
    where
        N: Spanned
            + VisitMutWith<Box<dyn VisitMut + Send + Sync>>
            + for<'aa> VisitMutWith<ApplyVisitors<'aa>>,
    {
        let span = n.span();

        if let Some(children) = self.visitors.get(&span) {
            for child in children.iter() {
                if self.index == child.0.len() - 1 {
                    if child.0.last() == Some(&span) {
                        n.visit_mut_with(&mut child.1());
                    }
                } else {
                    debug_assert!(self.index < child.0.len());

                    let mut children_map = HashMap::<_, Vec<_>>::with_capacity(child.0.len());
                    for span in child.0.iter().copied() {
                        children_map
                            .entry(span)
                            .or_default()
                            .push((child.0, child.1));
                    }

                    // Instead of resetting, we create a new instance of this struct
                    n.visit_mut_children_with(&mut ApplyVisitors {
                        visitors: children_map,
                        index: self.index + 1,
                    });
                }
            }
        }
    }
}

macro_rules! method {
    ($name:ident,$T:ty) => {
        fn $name(&mut self, n: &mut $T) {
            self.visit_if_required(n);
        }
    };
}

impl VisitMut for ApplyVisitors<'_> {
    noop_visit_mut_type!();

    method!(visit_mut_prop, Prop);
    method!(visit_mut_expr, Expr);
    method!(visit_mut_pat, Pat);
    method!(visit_mut_stmt, Stmt);
    method!(visit_mut_module_decl, ModuleDecl);
}

pub struct VisitWithPath<V>
where
    V: CreateVisitorFn,
{
    spans: Vec<Span>,
    creator: V,
    visitors: Vec<(Vec<Span>, VisitorFn)>,
}

pub trait CreateVisitorFn {
    fn create_visitor_fn(&mut self, ast_path: &[Span]) -> Option<VisitorFn>;
}

macro_rules! visit_rule {
    ($name:ident,$T:ty) => {
        fn $name(&mut self, n: &$T) {
            self.check(n);
        }
    };
}

impl<V> VisitWithPath<V>
where
    V: CreateVisitorFn,
{
    fn check<N>(&mut self, n: &N)
    where
        N: VisitWith<Self> + Spanned,
    {
        let span = n.span();

        self.spans.push(span);
        let v = self.creator.create_visitor_fn(&self.spans);
        if let Some(v) = v {
            self.visitors.push((self.spans.clone(), v));
        }

        n.visit_children_with(self);

        self.spans.pop();
    }
}

impl<V> Visit for VisitWithPath<V>
where
    V: CreateVisitorFn,
{
    noop_visit_type!();

    visit_rule!(visit_prop, Prop);
    visit_rule!(visit_expr, Expr);
    visit_rule!(visit_pat, Pat);
    visit_rule!(visit_stmt, Stmt);
    visit_rule!(visit_module_decl, ModuleDecl);
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use swc_common::{errors::HANDLER, BytePos, FileName, Mark, SourceFile, SourceMap, Span};
    use swc_ecma_transforms_base::resolver;
    use swc_ecmascript::{
        ast::*,
        parser::parse_file_as_module,
        visit::{noop_visit_mut_type, VisitMut, VisitMutWith},
    };

    use super::ApplyVisitors;

    fn parse(fm: &SourceFile) -> Module {
        let mut m = parse_file_as_module(
            &fm,
            Default::default(),
            EsVersion::latest(),
            None,
            &mut vec![],
        )
        .map_err(|err| HANDLER.with(|handler| err.into_diagnostic(&handler).emit()))
        .unwrap();

        let unresolved_mark = Mark::new();
        let top_level_mark = Mark::new();
        m.visit_mut_with(&mut resolver(unresolved_mark, top_level_mark, false));

        m
    }

    fn span_of(fm: &SourceFile, text: &str) -> Span {
        let idx = BytePos(fm.src.find(text).expect("span_of: text not found") as _);
        let lo = fm.start_pos + idx;

        Span::new(lo, lo + BytePos(text.len() as _), Default::default())
    }

    struct StrReplacer<'a> {
        from: &'a str,
        to: &'a str,
    }

    impl VisitMut for StrReplacer<'_> {
        noop_visit_mut_type!();

        fn visit_mut_str(&mut self, s: &mut Str) {
            s.value = s.value.replace(self.from, self.to).into();
            s.raw = None;
        }
    }

    fn replacer(from: &'static str, to: &'static str) -> super::VisitorFn {
        box || {
            eprintln!("Creating replacer");
            box StrReplacer { from, to }
        }
    }

    #[test]
    fn case_1() {
        testing::run_test(false, |cm, handler| {
            let fm = cm.new_source_file(FileName::Anon, "('foo', 'bar', ['baz']);".into());

            let m = parse(&fm);

            let bar_span = span_of(&fm, "'bar'");

            let stmt_span = span_of(&fm, "('foo', 'bar', ['baz']);");
            let expr_span = span_of(&fm, "('foo', 'bar', ['baz'])");
            let seq_span = span_of(&fm, "'foo', 'bar', ['baz']");
            let arr_span = span_of(&fm, "['baz']");
            let baz_span = span_of(&fm, "'baz'");

            dbg!(bar_span);
            dbg!(expr_span);
            dbg!(arr_span);
            dbg!(baz_span);

            {
                let mut map = HashMap::<_, Vec<_>>::default();

                let bar_span_vec = vec![stmt_span, expr_span, seq_span, bar_span];
                let bar_replacer = replacer("bar", "bar-success");
                {
                    let e = map.entry(stmt_span).or_default();

                    e.push((&bar_span_vec, &bar_replacer));
                }

                let mut m = m.clone();
                m.visit_mut_with(&mut ApplyVisitors::new(map));

                let s = format!("{:?}", m);
                assert!(s.contains("bar-success"), "Should be replaced: {:#?}", m);
            }

            {
                let mut map = HashMap::<_, Vec<_>>::default();

                let wrong_span_vec = vec![baz_span];
                let bar_replacer = replacer("bar", "bar-success");
                {
                    let e = map.entry(stmt_span).or_default();

                    e.push((&wrong_span_vec, &bar_replacer));
                }

                let mut m = m.clone();
                m.visit_mut_with(&mut ApplyVisitors::new(map));

                let s = format!("{:?}", m);
                assert!(
                    !s.contains("bar-success"),
                    "Should not be replaced: {:#?}",
                    m
                );
            }

            Ok(())
        })
        .unwrap();
    }
}
