# CubeHarvest: Cluster Frontier

> A resource management simulation where players strategically deploy and scale "Astro-Units" across vibrant "Astro-Nodes" to mine valuable cosmic resources, build a resilient "Galactic Grid," and outmaneuver chaotic space events to dominate the intergalactic market.

## Concept Overview

"CubeHarvest: Cluster Frontier" reimagines a Kubernetes cluster as a living, breathing interstellar mining operation. Players take on the role of a "Galactic Grid Administrator," tasked with orchestrating a fleet of automated "Astro-Units" (Pods) across various celestial "Astro-Nodes" (Kubernetes Nodes). The core gameplay revolves around declarative management: players define *what* they want their grid to achieve, and the underlying systems automatically handle the how. The goal is to accumulate wealth ("Credits") by creating efficient resource processing pipelines, expand the grid, and survive chaotic events.

This game directly interacts with a real Kubernetes cluster. The "Astro-Units" you create are actual Pods, and the "Astro-Nodes" are your cluster's Nodes.

## Gameplay

The game loop involves:
1.  **Observing** your cluster's state.
2.  **Deploying** "Miner" and "Processor" units to generate credits.
3.  **Earning** credits from efficient miner-processor pairs.
4.  **Spending** credits to deploy more units.
5.  **Surviving** random pod deletions representing "Cosmic Volatility Events".

### Controls

The game is controlled via the keyboard.

-   **Main Menu:**
    -   `Space`: Start the game.
    -   `Escape`: Exit.
-   **Cluster View (Main Game Screen):**
    -   `←` / `→`: Switch between Astro-Nodes.
    -   `Enter`: Select the current Astro-Node (feature in development).
    -   `C`: Enter Create mode to deploy a new Astro-Unit.
-   **Create Mode:**
    -   `M`: Choose to create a **Miner** unit.
    -   `P`: Choose to create a **Processor** unit.
    -   After selecting a unit type:
        -   **(Miner only)** Type the target IP address of a Processor unit.
        -   `Enter`: Deploy the unit.
        -   `Backspace`: Delete the last character of the IP.
    -   `Escape`: Go back to the Cluster View.

## Technical Stack

-   **Language:** [Rust](https://www.rust-lang.org/)
-   **Game Engine:** [macroquad](https://macroquad.rs/)
-   **Kubernetes Interaction:** [kube-rs](https://kube.rs/)
-   **Async Runtime:** [Tokio](https://tokio.rs/)
-   **Templating:** [Askama](https://github.com/djc/askama) for generating Pod manifests.

## How to Run

> **⚠️ WARNING: Do Not Run Against a Production Cluster!**
> This game is designed to interact with a Kubernetes cluster by creating and deleting resources (Pods). Running it against a production or important non-production cluster can lead to data loss or service disruption. Please use a dedicated, temporary, or simulated cluster for playing this game. We strongly recommend using the provided `kwok` setup.

This game interacts with a real Kubernetes cluster. For a quick and lightweight setup, we recommend using `kwok` to simulate a cluster.

### Prerequisites

1.  [Rust toolchain](https://www.rust-lang.org/tools/install)
2.  [just](https://github.com/casey/just), a command runner.
3.  [kwok](https://kwok.sigs.k8s.io/docs/user/installation/), a toolkit to simulate thousands of Nodes and Pods.

### Setup & Run

1.  **Clone the repository:**
    ```bash
    git clone <repository-url>
    cd cube-harvest
    ```
2.  **Create a simulated cluster:**
    This command uses `kwok` to create a new cluster with 3 nodes.
    ```bash
    just create-cluster
    ```
3.  **Run the game:**
    ```bash
    cargo run
    ```
    The game window will open and connect to your `kwok` cluster.
4.  **(Optional) Clean up:**
    When you are done, you can delete the simulated cluster.
    ```bash
    just delete-cluster
    ```

## Game Design Document

For a deeper dive into the game's mechanics, lore, and future plans, please see the full [Game Design Document](./docs/GDD.md).
