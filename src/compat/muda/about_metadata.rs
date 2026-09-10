//! About-panel metadata (spec `02` §4.6), mirroring muda's `about_metadata`
//! module. Used by [`PredefinedMenuItem::about`](super::PredefinedMenuItem::about);
//! the standard About panel is populated from these fields in a native menu bar.

/// Metadata shown in the standard About panel, mirroring muda's `AboutMetadata`.
#[derive(Clone, Debug, Default)]
pub struct AboutMetadata {
    /// The application name.
    pub name: Option<String>,
    /// The application version.
    pub version: Option<String>,
    /// A short version string.
    pub short_version: Option<String>,
    /// A copyright line.
    pub copyright: Option<String>,
    /// The authors.
    pub authors: Option<Vec<String>>,
    /// A comments / description line.
    pub comments: Option<String>,
    /// A website URL.
    pub website: Option<String>,
}

/// A builder for [`AboutMetadata`], mirroring muda's `AboutMetadataBuilder`.
#[derive(Clone, Debug, Default)]
pub struct AboutMetadataBuilder {
    metadata: AboutMetadata,
}

impl AboutMetadataBuilder {
    /// A new, empty builder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the application name.
    pub fn name(mut self, name: Option<impl Into<String>>) -> Self {
        self.metadata.name = name.map(Into::into);
        self
    }

    /// Set the application version.
    pub fn version(mut self, version: Option<impl Into<String>>) -> Self {
        self.metadata.version = version.map(Into::into);
        self
    }

    /// Set the short version string.
    pub fn short_version(mut self, short_version: Option<impl Into<String>>) -> Self {
        self.metadata.short_version = short_version.map(Into::into);
        self
    }

    /// Set the copyright line.
    pub fn copyright(mut self, copyright: Option<impl Into<String>>) -> Self {
        self.metadata.copyright = copyright.map(Into::into);
        self
    }

    /// Set the authors.
    pub fn authors(mut self, authors: Option<Vec<String>>) -> Self {
        self.metadata.authors = authors;
        self
    }

    /// Set the comments / description line.
    pub fn comments(mut self, comments: Option<impl Into<String>>) -> Self {
        self.metadata.comments = comments.map(Into::into);
        self
    }

    /// Set the website URL.
    pub fn website(mut self, website: Option<impl Into<String>>) -> Self {
        self.metadata.website = website.map(Into::into);
        self
    }

    /// Finish building the metadata.
    pub fn build(self) -> AboutMetadata {
        self.metadata
    }
}
