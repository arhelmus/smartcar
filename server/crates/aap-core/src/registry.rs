//! Service registry: maps [`ChannelId`] → [`Service`] implementation.

use std::collections::HashMap;

use aap_contracts::{ChannelId, Service, ServiceDescriptor};

/// Owns all per-channel [`Service`] instances for one connection.
///
/// Register services before constructing a [`crate::Connection`]; the
/// connection takes ownership of the registry and consults it during the
/// service-discovery and channel-open phases.
pub struct ServiceRegistry {
    services: HashMap<ChannelId, Box<dyn Service>>,
}

impl ServiceRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            services: HashMap::new(),
        }
    }

    /// Register a service.
    ///
    /// The service's [`Service::channel`] value is used as the map key.
    /// Registering a second service on the same channel replaces the first.
    pub fn register(&mut self, service: impl Service + 'static) {
        self.services.insert(service.channel(), Box::new(service));
    }

    /// Look up a mutable reference to the service bound to `id`.
    pub fn get_mut(&mut self, id: ChannelId) -> Option<&mut Box<dyn Service>> {
        self.services.get_mut(&id)
    }

    /// Return a [`ServiceDescriptor`] for every registered service.
    ///
    /// The descriptors are used to build the `ServiceDiscoveryResponse` sent
    /// to the head unit during protocol setup.
    pub fn descriptors(&self) -> Vec<ServiceDescriptor> {
        self.services.values().map(|s| s.descriptor()).collect()
    }
}

impl Default for ServiceRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use bytes::Bytes;

    use aap_contracts::{ChannelId, Frame, ServiceDescriptor, ServiceError};

    use super::*;

    /// Minimal mock service for testing the registry.
    struct MockService {
        channel: ChannelId,
    }

    #[async_trait]
    impl aap_contracts::Service for MockService {
        fn channel(&self) -> ChannelId {
            self.channel
        }

        fn descriptor(&self) -> ServiceDescriptor {
            ServiceDescriptor {
                channel: self.channel,
                descriptor_bytes: Bytes::from_static(b"mock"),
            }
        }

        async fn handle(
            &mut self,
            _message_id: u16,
            _payload: Bytes,
        ) -> Result<Vec<Frame>, ServiceError> {
            Ok(vec![])
        }
    }

    #[test]
    fn register_and_descriptors() {
        let mut registry = ServiceRegistry::new();
        assert!(registry.descriptors().is_empty());

        registry.register(MockService {
            channel: ChannelId::Video,
        });

        let descs = registry.descriptors();
        assert_eq!(descs.len(), 1);
        assert_eq!(descs[0].channel, ChannelId::Video);
        assert_eq!(descs[0].descriptor_bytes, Bytes::from_static(b"mock"));
    }

    #[test]
    fn get_mut_returns_correct_service() {
        let mut registry = ServiceRegistry::new();
        registry.register(MockService {
            channel: ChannelId::Sensor,
        });

        assert!(registry.get_mut(ChannelId::Sensor).is_some());
        assert!(registry.get_mut(ChannelId::Video).is_none());
    }

    #[test]
    fn register_replaces_existing() {
        let mut registry = ServiceRegistry::new();
        registry.register(MockService {
            channel: ChannelId::MediaSink,
        });
        registry.register(MockService {
            channel: ChannelId::MediaSink,
        });
        assert_eq!(registry.descriptors().len(), 1);
    }
}
