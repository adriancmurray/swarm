/* ==========================================================================
   swarm — Interactive Marketing Site Script
   ========================================================================== */

document.addEventListener('DOMContentLoaded', () => {
  
  // ---------------------------------------------------------------------------
  // 1. Sticky Navigation Scroll Handler
  // ---------------------------------------------------------------------------
  const navbar = document.getElementById('navbar');
  const handleScroll = () => {
    if (window.scrollY > 40) {
      navbar.classList.add('scrolled');
    } else {
      navbar.classList.remove('scrolled');
    }
  };
  window.addEventListener('scroll', handleScroll);
  handleScroll(); // Initial check

  // ---------------------------------------------------------------------------
  // 2. Mobile Menu Toggle
  // ---------------------------------------------------------------------------
  const mobileToggle = document.getElementById('mobile-toggle');
  const navLinks = document.querySelector('.nav-links');

  if (mobileToggle && navLinks) {
    mobileToggle.addEventListener('click', (e) => {
      e.stopPropagation();
      navLinks.classList.toggle('mobile-open');
      mobileToggle.classList.toggle('active');
    });

    // Close menu when clicking outside or on a link
    document.addEventListener('click', (e) => {
      if (!navLinks.contains(e.target) && e.target !== mobileToggle) {
        navLinks.classList.remove('mobile-open');
        mobileToggle.classList.remove('active');
      }
    });

    navLinks.querySelectorAll('a').forEach(link => {
      link.addEventListener('click', () => {
        navLinks.classList.remove('mobile-open');
        mobileToggle.classList.remove('active');
      });
    });
  }

  // ---------------------------------------------------------------------------
  // 3. Interactive Terminal Playground Tabs
  // ---------------------------------------------------------------------------
  const tabButtons = document.querySelectorAll('.tab-btn');
  const tabPanes = document.querySelectorAll('.tab-pane');

  tabButtons.forEach(button => {
    button.addEventListener('click', () => {
      const targetTab = button.getAttribute('data-tab');

      // Deactivate all buttons & panes
      tabButtons.forEach(btn => btn.classList.remove('active'));
      tabPanes.forEach(pane => pane.classList.remove('active'));

      // Activate clicked button & corresponding pane
      button.classList.add('active');
      const targetPane = document.getElementById(`pane-${targetTab}`);
      if (targetPane) {
        targetPane.classList.add('active');
      }
    });
  });

  // ---------------------------------------------------------------------------
  // 4. Code Blocks Copy to Clipboard
  // ---------------------------------------------------------------------------
  const copyButtons = document.querySelectorAll('.copy-button');

  copyButtons.forEach(button => {
    button.addEventListener('click', async () => {
      const textToCopy = button.getAttribute('data-copy');
      if (!textToCopy) return;

      try {
        await navigator.clipboard.writeText(textToCopy);
        
        // Visual feedback state
        button.classList.add('copied');
        button.textContent = 'Copied!';

        setTimeout(() => {
          button.classList.remove('copied');
          button.textContent = 'Copy';
        }, 2000);
      } catch (err) {
        console.error('Failed to copy text: ', err);
        button.textContent = 'Error';
        setTimeout(() => {
          button.textContent = 'Copy';
        }, 2000);
      }
    });
  });

  // ---------------------------------------------------------------------------
  // 5. Crate Architecture Interactive Details Panel
  // ---------------------------------------------------------------------------
  const crateNodes = document.querySelectorAll('.crate-node');
  const detailsTitle = document.getElementById('details-crate-title');
  const detailsDesc = document.getElementById('details-crate-desc');
  const detailsMeta = document.getElementById('details-crate-meta');
  const detailsDeps = document.getElementById('details-crate-deps');

  // Crate metadata dictionary
  const crateData = {
    contracts: {
      title: 'swarm-contracts',
      desc: 'Provides wire-stable serialization contracts (IDs, events, jobs, and telemetry payloads) shared across the workspace. It compiles extremely fast and depends on nothing but serde.',
      deps: 'serde, serde_json'
    },
    core: {
      title: 'swarm-core',
      desc: 'Contains repository traits and pure domain substrate for jobs, sessions, events, ledgers, telemetry, and liveness.',
      deps: 'swarm-contracts, thiserror'
    },
    store: {
      title: 'swarm-store',
      desc: 'Implements concrete filesystem-backed repositories for jobs, sessions, events, ledgers, telemetry, and monitor state.',
      deps: 'swarm-contracts, swarm-core, serde_json'
    },
    kernel: {
      title: 'swarm-kernel',
      desc: 'Houses stateless leaf components such as config, backend descriptors, process helpers, routing, profiles, IDs, and formatting.',
      deps: 'swarm-contracts, thiserror'
    },
    exec: {
      title: 'swarm-exec',
      desc: 'The central engine of the orchestrator. Implements execution, orchestration, synthesis, sessions, backend registries, and monitors.',
      deps: 'swarm-contracts, swarm-core, swarm-store, swarm-kernel'
    },
    cli: {
      title: 'swarm-cli',
      desc: 'The CLI command layer. Dispatches commands for runs, swarms, discussions, sessions, monitors, MCP, and backend scaffolding.',
      deps: 'swarm-exec, swarm-kernel, swarm-mcp, swarm-store'
    },
    mcp: {
      title: 'swarm-mcp',
      desc: 'Implements the MCP server layer, schema generation, manifests, reports, overview helpers, dispatch, and registry hooks.',
      deps: 'swarm-exec, swarm-kernel, swarm-store, serde_json'
    },
    manager: {
      title: 'swarm-manager',
      desc: 'A native single-agent harness with provider registry, encrypted credential vault, presets, tools, and an in-process agent loop behind feature flags.',
      deps: 'rusqlite, keyring, chacha20poly1305, reqwest'
    },
    registrar: {
      title: 'swarm-registrar',
      desc: 'An optional generic JSON service-registry hook for integrations that want simple registration plumbing.',
      deps: 'serde, serde_json'
    }
  };

  const updateDetailsPanel = (crateKey) => {
    const data = crateData[crateKey];
    if (!data) return;

    // Remove active state from other nodes
    crateNodes.forEach(node => node.classList.remove('active-node'));

    // Add active state to hovered node
    const activeNode = document.querySelector(`[data-crate="${crateKey}"]`);
    if (activeNode) {
      activeNode.classList.add('active-node');
    }

    // Update details card content
    detailsTitle.textContent = data.title;
    detailsDesc.textContent = data.desc;
    detailsDeps.textContent = data.deps;
    detailsMeta.classList.remove('hidden');
  };

  // Add hover & click listeners to crate nodes
  crateNodes.forEach(node => {
    const crateKey = node.getAttribute('data-crate');
    
    node.addEventListener('mouseenter', () => {
      updateDetailsPanel(crateKey);
    });

    node.addEventListener('click', (e) => {
      e.preventDefault();
      updateDetailsPanel(crateKey);
      
      // Scroll details panel into view on mobile
      if (window.innerWidth <= 991) {
        document.getElementById('crate-details-panel').scrollIntoView({ behavior: 'smooth', block: 'nearest' });
      }
    });
  });

  // Set default active node (e.g. exec)
  updateDetailsPanel('exec');

});
