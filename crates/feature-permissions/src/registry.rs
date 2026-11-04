//! Registration is a trusted Host operation. Wire declarations cannot construct
//! handles. A handle is bound to one registry instance and one registration, not
//! merely to matching Workspace/Feature strings. Reinstallation retires old handles.

use crate::types::*;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum PermissionError {
    #[error("handle belongs to another Host or a retired registration")]
    ForeignHandle,
    #[error("Feature already registered: {0}")]
    FeatureCollision(FeatureIdentity),
    #[error("namespace already owned: {0}")]
    NamespaceCollision(NamespaceId),
    #[error("declaration does not belong to namespace: {0}")]
    Ownership(PermissionKey),
    #[error("duplicate declaration: {0}")]
    Duplicate(String),
    #[error("unknown key: {0}")]
    UnknownKey(PermissionKey),
    #[error("unknown set: {0}")]
    UnknownSet(PermissionSetId),
    #[error("cycle in permission graph: {0}")]
    Cycle(String),
    #[error("resolution is stale; resolve current policy again")]
    StaleResolution,
    #[error("requested delegation exceeds current ceiling: {0}")]
    DelegationExceeded(PermissionKey),
    #[error("limited grant has been revoked")]
    RevokedGrant,
    #[error("Job grant belongs to another Job")]
    WrongJob,
}

#[derive(Clone, Debug)]
pub struct FeatureHandle {
    host: Arc<()>,
    registration: Arc<()>,
    identity: FeatureIdentity,
}
#[derive(Clone, Debug)]
pub struct NamespaceHandle {
    feature: FeatureHandle,
    namespace: NamespaceId,
}
impl NamespaceHandle {
    pub fn namespace(&self) -> &NamespaceId {
        &self.namespace
    }

    /// Compare the actual registrations, never caller-supplied owner names.
    pub fn is_owned_by(&self, feature: &FeatureHandle) -> bool {
        Arc::ptr_eq(&self.feature.host, &feature.host)
            && Arc::ptr_eq(&self.feature.registration, &feature.registration)
    }
}
#[derive(Clone, Debug)]
struct FeatureRecord {
    registration: Arc<()>,
    enabled: bool,
}
#[derive(Clone, Debug)]
struct NamespaceRecord {
    owner: FeatureIdentity,
    declaration: NamespaceDeclaration,
}

/// Not deserializable: a management projection cannot become an authorization
/// result. Results must be re-resolved after *any* registry change.
#[derive(Debug)]
pub struct ResolvedPermissions {
    host: Arc<()>,
    catalog: Arc<RegistryState>,
    projection: PermissionResolution,
}
impl ResolvedPermissions {
    pub fn projection(&self) -> &PermissionResolution {
        &self.projection
    }
}

/// Keys are expanded once at issuance and never expanded again. Thus later
/// children/set additions cannot enlarge an already issued child grant.
#[derive(Debug)]
pub struct LimitedGrant {
    host: Arc<()>,
    keys: BTreeSet<PermissionKey>,
    registrations: BTreeMap<PermissionKey, Arc<()>>,
    revoked: bool,
}
impl LimitedGrant {
    pub fn keys(&self) -> &BTreeSet<PermissionKey> {
        &self.keys
    }
    pub fn revoke(&mut self) {
        self.revoked = true;
    }
}

/// A Job's operation grant is an additional limit, not an alternative policy.
/// The caller supplies the authenticated Job identity at issuance and use; this
/// does not validate attempts, writers, deadlines or provider restrictions.
#[derive(Debug)]
pub struct JobGrant {
    job: String,
    limit: LimitedGrant,
}
impl JobGrant {
    pub fn revoke(&mut self) {
        self.limit.revoke();
    }
}

pub struct PermissionHost {
    identity: Arc<()>,
    workspace: WorkspaceId,
    catalog: Arc<RegistryState>,
}

#[derive(Clone, Debug, Default)]
struct RegistryState {
    features: BTreeMap<FeatureIdentity, FeatureRecord>,
    namespaces: BTreeMap<NamespaceId, NamespaceRecord>,
}

impl PermissionHost {
    /// Constructed by the trusted Workspace Host, not from a Feature's input.
    pub fn new(workspace: WorkspaceId) -> Self {
        Self {
            identity: Arc::new(()),
            workspace,
            catalog: Arc::new(RegistryState::default()),
        }
    }

    /// Only the trusted loader calls this after establishing the actual builtin
    /// or trusted Plugin identity. There is no Feature-facing raw-owner lookup.
    pub fn register_feature(
        &mut self,
        identity: FeatureIdentity,
    ) -> Result<FeatureHandle, PermissionError> {
        if self.catalog.features.contains_key(&identity) {
            return Err(PermissionError::FeatureCollision(identity));
        }
        let registration = Arc::new(());
        Arc::make_mut(&mut self.catalog).features.insert(
            identity.clone(),
            FeatureRecord {
                registration: registration.clone(),
                enabled: true,
            },
        );
        Ok(FeatureHandle {
            host: self.identity.clone(),
            registration,
            identity,
        })
    }

    fn check_feature(&self, feature: &FeatureHandle) -> Result<(), PermissionError> {
        if !Arc::ptr_eq(&self.identity, &feature.host)
            || !self
                .catalog
                .features
                .get(&feature.identity)
                .is_some_and(|r| Arc::ptr_eq(&r.registration, &feature.registration))
        {
            return Err(PermissionError::ForeignHandle);
        }
        Ok(())
    }
    fn check_namespace(&self, handle: &NamespaceHandle) -> Result<(), PermissionError> {
        self.check_feature(&handle.feature)?;
        if !self
            .catalog
            .namespaces
            .get(&handle.namespace)
            .is_some_and(|r| r.owner == handle.feature.identity)
        {
            return Err(PermissionError::ForeignHandle);
        }
        Ok(())
    }

    pub fn bind_namespace(
        &mut self,
        feature: &FeatureHandle,
        namespace: NamespaceId,
    ) -> Result<NamespaceHandle, PermissionError> {
        self.check_feature(feature)?;
        if self.catalog.namespaces.contains_key(&namespace) {
            return Err(PermissionError::NamespaceCollision(namespace));
        }
        Arc::make_mut(&mut self.catalog).namespaces.insert(
            namespace.clone(),
            NamespaceRecord {
                owner: feature.identity.clone(),
                declaration: NamespaceDeclaration {
                    permissions: vec![],
                    sets: vec![],
                },
            },
        );
        Ok(NamespaceHandle {
            feature: feature.clone(),
            namespace,
        })
    }

    /// Replace atomically. All keys and set contributions must belong to this
    /// namespace. References to existing other namespaces do not transfer ownership.
    pub fn declare(
        &mut self,
        handle: &NamespaceHandle,
        declaration: NamespaceDeclaration,
    ) -> Result<(), PermissionError> {
        self.check_namespace(handle)?;
        let mut keys = BTreeSet::new();
        for definition in &declaration.permissions {
            if definition.key.namespace() != handle.namespace {
                return Err(PermissionError::Ownership(definition.key.clone()));
            }
            if !keys.insert(definition.key.clone()) {
                return Err(PermissionError::Duplicate(definition.key.to_string()));
            }
            let mut children = BTreeSet::new();
            for child in &definition.children {
                if !children.insert(child) {
                    return Err(PermissionError::Duplicate(child.to_string()));
                }
            }
        }
        let mut sets = BTreeSet::new();
        for contribution in &declaration.sets {
            if !sets.insert(&contribution.set) {
                return Err(PermissionError::Duplicate(contribution.set.to_string()));
            }
            for reference in &contribution.children {
                if let PermissionReference::Namespace(ns) = reference {
                    if ns.namespace != handle.namespace {
                        return Err(PermissionError::ForeignHandle);
                    }
                    for key in &ns.keys {
                        if key.namespace() != ns.namespace {
                            return Err(PermissionError::Ownership(key.clone()));
                        }
                    }
                }
            }
        }
        let mut candidate = self.catalog.namespaces.clone();
        candidate
            .get_mut(&handle.namespace)
            .expect("checked namespace")
            .declaration = declaration;
        validate_graph(&candidate)?;
        Arc::make_mut(&mut self.catalog).namespaces = candidate;
        Ok(())
    }

    pub fn set_enabled(
        &mut self,
        feature: &FeatureHandle,
        enabled: bool,
    ) -> Result<(), PermissionError> {
        self.check_feature(feature)?;
        if self.catalog.features[&feature.identity].enabled != enabled {
            Arc::make_mut(&mut self.catalog)
                .features
                .get_mut(&feature.identity)
                .expect("checked feature")
                .enabled = enabled;
        }
        Ok(())
    }

    /// Removal withdraws only this Feature's definitions/contributions. Dangling
    /// references remain diagnostic and fail closed at resolution, never cached.
    pub fn remove_feature(&mut self, feature: &FeatureHandle) -> Result<(), PermissionError> {
        self.check_feature(feature)?;
        Arc::make_mut(&mut self.catalog)
            .features
            .remove(&feature.identity);
        Arc::make_mut(&mut self.catalog)
            .namespaces
            .retain(|_, r| r.owner != feature.identity);
        Ok(())
    }

    pub fn catalog(&self) -> PermissionCatalog {
        PermissionCatalog {
            workspace: self.workspace.clone(),
            namespaces: self
                .catalog
                .namespaces
                .iter()
                .map(|(ns, r)| NamespaceCatalog {
                    namespace: ns.clone(),
                    feature: r.owner.clone(),
                    enabled: self.catalog.features[&r.owner].enabled,
                    declaration: r.declaration.clone(),
                })
                .collect(),
        }
    }

    pub fn resolve(&self, assignment: Option<&PermissionAssignment>) -> ResolvedPermissions {
        let mut projection = PermissionResolution {
            workspace: self.workspace.clone(),
            allowed: BTreeMap::new(),
            diagnostics: vec![],
        };
        match assignment {
            None => projection.diagnostics.push(PermissionDiagnostic::Unset),
            Some(a) if a.revoked => projection.diagnostics.push(PermissionDiagnostic::Revoked),
            Some(a) => {
                let mut expansion = Expansion::new(self);
                for reference in &a.grants {
                    expansion.reference(reference, vec![]);
                }
                let mut exclusions = Expansion::new(self);
                for reference in &a.exclusions {
                    exclusions.reference(reference, vec![]);
                }
                for key in exclusions.allowed.keys() {
                    if expansion.allowed.remove(key).is_some() {
                        expansion
                            .diagnostics
                            .insert(PermissionDiagnostic::Excluded(key.clone()));
                    }
                }
                expansion.diagnostics.extend(exclusions.diagnostics);
                // Invalid references, including malformed namespace/key pairings,
                // invalidate the assignment instead of partially accepting a typo.
                if !expansion.diagnostics.iter().any(|d| {
                    matches!(
                        d,
                        PermissionDiagnostic::UnknownKey(_)
                            | PermissionDiagnostic::UnknownSet(_)
                            | PermissionDiagnostic::UnknownNamespace(_)
                    )
                }) {
                    projection.allowed = expansion
                        .allowed
                        .into_iter()
                        .map(|(key, origins)| (key, origins.into_iter().collect()))
                        .collect();
                }
                projection.diagnostics = expansion.diagnostics.into_iter().collect();
            }
        }
        ResolvedPermissions {
            host: self.identity.clone(),
            catalog: self.catalog.clone(),
            projection,
        }
    }

    fn check_resolution(&self, resolved: &ResolvedPermissions) -> Result<(), PermissionError> {
        if !Arc::ptr_eq(&self.identity, &resolved.host) {
            return Err(PermissionError::ForeignHandle);
        }
        if !Arc::ptr_eq(&self.catalog, &resolved.catalog) {
            return Err(PermissionError::StaleResolution);
        }
        Ok(())
    }

    /// A key permission is only one conjunct. The execution boundary must still
    /// check path/provider/context/domain restrictions at the actual operation.
    pub fn allows(
        &self,
        namespace: &NamespaceHandle,
        resolved: &ResolvedPermissions,
        key: &PermissionKey,
    ) -> Result<bool, PermissionError> {
        self.check_namespace(namespace)?;
        self.check_resolution(resolved)?;
        if key.namespace() != namespace.namespace {
            return Err(PermissionError::Ownership(key.clone()));
        }
        Ok(resolved.projection.allowed.contains_key(key))
    }

    pub fn issue_delegation(
        &self,
        ceiling: &ResolvedPermissions,
        requested: &[PermissionKey],
    ) -> Result<LimitedGrant, PermissionError> {
        self.check_resolution(ceiling)?;
        let mut expansion = Expansion::new(self);
        for key in requested {
            expansion.key(key, vec![]);
        }
        for diagnostic in &expansion.diagnostics {
            if let PermissionDiagnostic::UnknownKey(key) = diagnostic {
                return Err(PermissionError::UnknownKey(key.clone()));
            }
        }
        for key in expansion.allowed.keys() {
            if !ceiling.projection.allowed.contains_key(key) {
                return Err(PermissionError::DelegationExceeded(key.clone()));
            }
        }
        Ok(LimitedGrant {
            host: self.identity.clone(),
            registrations: expansion
                .allowed
                .keys()
                .map(|key| {
                    let owner = &self.catalog.namespaces[&key.namespace()].owner;
                    (
                        key.clone(),
                        self.catalog.features[owner].registration.clone(),
                    )
                })
                .collect(),
            keys: expansion.allowed.into_keys().collect(),
            revoked: false,
        })
    }

    pub fn issue_job_grant(
        &self,
        job: String,
        ceiling: &ResolvedPermissions,
        requested: &[PermissionKey],
    ) -> Result<JobGrant, PermissionError> {
        Ok(JobGrant {
            job,
            limit: self.issue_delegation(ceiling, requested)?,
        })
    }

    fn grant_key_is_current(&self, grant: &LimitedGrant, key: &PermissionKey) -> bool {
        let Some(record) = self.catalog.namespaces.get(&key.namespace()) else {
            return false;
        };
        grant.registrations.get(key).is_some_and(|registration| {
            let feature = &self.catalog.features[&record.owner];
            feature.enabled && Arc::ptr_eq(registration, &feature.registration)
        })
    }

    fn check_grant(&self, grant: &LimitedGrant) -> Result<(), PermissionError> {
        if !Arc::ptr_eq(&self.identity, &grant.host) {
            return Err(PermissionError::ForeignHandle);
        }
        if grant.revoked {
            return Err(PermissionError::RevokedGrant);
        }
        Ok(())
    }

    /// Parent execution, parent delegation and actually passed keys all constrain
    /// the child. This exact-set intersection performs no children expansion.
    pub fn resolve_limited(
        &self,
        execution: &ResolvedPermissions,
        delegation: &ResolvedPermissions,
        grant: &LimitedGrant,
    ) -> Result<ResolvedPermissions, PermissionError> {
        self.check_resolution(execution)?;
        self.check_resolution(delegation)?;
        self.check_grant(grant)?;
        let mut projection = execution.projection.clone();
        projection.allowed.retain(|key, _| {
            grant.keys.contains(key)
                && self.grant_key_is_current(grant, key)
                && delegation.projection.allowed.contains_key(key)
        });
        Ok(ResolvedPermissions {
            host: self.identity.clone(),
            catalog: self.catalog.clone(),
            projection,
        })
    }

    pub fn resolve_job(
        &self,
        job: &str,
        execution: &ResolvedPermissions,
        delegation: &ResolvedPermissions,
        child: &LimitedGrant,
        grant: &JobGrant,
    ) -> Result<ResolvedPermissions, PermissionError> {
        if grant.job != job {
            return Err(PermissionError::WrongJob);
        }
        self.check_grant(&grant.limit)?;
        let mut result = self.resolve_limited(execution, delegation, child)?;
        result.projection.allowed.retain(|key, _| {
            grant.limit.keys.contains(key) && self.grant_key_is_current(&grant.limit, key)
        });
        Ok(result)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Node {
    Key(PermissionKey),
    Set(PermissionSetId),
}

fn graph(
    namespaces: &BTreeMap<NamespaceId, NamespaceRecord>,
) -> Result<BTreeMap<Node, Vec<Node>>, PermissionError> {
    let mut nodes = BTreeMap::new();
    for r in namespaces.values() {
        for d in &r.declaration.permissions {
            nodes.insert(
                Node::Key(d.key.clone()),
                d.children.iter().cloned().map(Node::Key).collect(),
            );
        }
        for s in &r.declaration.sets {
            let edges = nodes
                .entry(Node::Set(s.set.clone()))
                .or_insert_with(Vec::new);
            for reference in &s.children {
                match reference {
                    PermissionReference::Set(set) => edges.push(Node::Set(set.clone())),
                    PermissionReference::Namespace(ns) => {
                        for key in &ns.keys {
                            if key.namespace() != ns.namespace {
                                return Err(PermissionError::Ownership(key.clone()));
                            }
                            edges.push(Node::Key(key.clone()));
                        }
                    }
                }
            }
        }
    }
    Ok(nodes)
}

fn validate_graph(
    namespaces: &BTreeMap<NamespaceId, NamespaceRecord>,
) -> Result<(), PermissionError> {
    fn visit(
        node: &Node,
        nodes: &BTreeMap<Node, Vec<Node>>,
        active: &mut BTreeSet<Node>,
        done: &mut BTreeSet<Node>,
    ) -> Result<(), PermissionError> {
        if done.contains(node) {
            return Ok(());
        }
        if !active.insert(node.clone()) {
            return Err(PermissionError::Cycle(format!("{node:?}")));
        }
        let edges = nodes.get(node).ok_or_else(|| match node {
            Node::Key(k) => PermissionError::UnknownKey(k.clone()),
            Node::Set(s) => PermissionError::UnknownSet(s.clone()),
        })?;
        for child in edges {
            visit(child, nodes, active, done)?;
        }
        active.remove(node);
        done.insert(node.clone());
        Ok(())
    }
    let nodes = graph(namespaces)?;
    let mut done = BTreeSet::new();
    for node in nodes.keys() {
        visit(node, &nodes, &mut BTreeSet::new(), &mut done)?;
    }
    Ok(())
}

struct Expansion<'a> {
    host: &'a PermissionHost,
    allowed: BTreeMap<PermissionKey, BTreeSet<Vec<OriginStep>>>,
    diagnostics: BTreeSet<PermissionDiagnostic>,
}
impl<'a> Expansion<'a> {
    fn new(host: &'a PermissionHost) -> Self {
        Self {
            host,
            allowed: BTreeMap::new(),
            diagnostics: BTreeSet::new(),
        }
    }
    fn reference(&mut self, reference: &PermissionReference, path: Vec<OriginStep>) {
        match reference {
            PermissionReference::Namespace(ns) => {
                if !self.host.catalog.namespaces.contains_key(&ns.namespace) {
                    self.diagnostics
                        .insert(PermissionDiagnostic::UnknownNamespace(ns.namespace.clone()));
                    return;
                }
                for key in &ns.keys {
                    if key.namespace() != ns.namespace {
                        self.diagnostics
                            .insert(PermissionDiagnostic::UnknownKey(key.clone()));
                    } else {
                        self.key(key, path.clone());
                    }
                }
            }
            PermissionReference::Set(set) => {
                // Cycles cannot be installed; the path guard also makes this
                // traversal robust to future graph-loading implementations.
                if path.contains(&OriginStep::Set(set.clone())) {
                    return;
                }
                let mut found = false;
                for record in self.host.catalog.namespaces.values() {
                    for contribution in &record.declaration.sets {
                        if &contribution.set != set {
                            continue;
                        }
                        found = true;
                        if !self.host.catalog.features[&record.owner].enabled {
                            self.diagnostics
                                .insert(PermissionDiagnostic::DisabledFeature(
                                    record.owner.clone(),
                                ));
                            continue;
                        }
                        let mut next = path.clone();
                        next.push(OriginStep::Set(set.clone()));
                        next.push(OriginStep::Feature(record.owner.clone()));
                        for child in &contribution.children {
                            self.reference(child, next.clone());
                        }
                    }
                }
                if !found {
                    self.diagnostics
                        .insert(PermissionDiagnostic::UnknownSet(set.clone()));
                }
            }
        }
    }
    fn key(&mut self, key: &PermissionKey, mut path: Vec<OriginStep>) {
        let Some(record) = self.host.catalog.namespaces.get(&key.namespace()) else {
            self.diagnostics
                .insert(PermissionDiagnostic::UnknownKey(key.clone()));
            return;
        };
        let Some(definition) = record
            .declaration
            .permissions
            .iter()
            .find(|d| &d.key == key)
        else {
            self.diagnostics
                .insert(PermissionDiagnostic::UnknownKey(key.clone()));
            return;
        };
        if !self.host.catalog.features[&record.owner].enabled {
            self.diagnostics
                .insert(PermissionDiagnostic::DisabledFeature(record.owner.clone()));
            return;
        }
        if path.contains(&OriginStep::Key(key.clone())) {
            return;
        }
        path.push(OriginStep::Feature(record.owner.clone()));
        path.push(OriginStep::Key(key.clone()));
        self.allowed
            .entry(key.clone())
            .or_default()
            .insert(path.clone());
        for child in &definition.children {
            self.key(child, path.clone());
        }
    }
}
