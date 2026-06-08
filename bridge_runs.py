import os
import subprocess
import json
import re
import shutil

binaries = [
    "drone_fso_cliff",
    "drone_energy_cliff",
    "drone_ew_boundary",
    "drone_swarm_topology",
    "drone_ew_166m_core",
    "pdop_smooth",
    "humanoid_friction_fix",
    "friis_asymmetric"
]

genesis_dir = "/Users/aijesusbro/Spectrum/G^G/genesis_core"
corpus_dir = "/Users/aijesusbro/Spectrum/data/corpus/SINGULARITY"

os.makedirs(corpus_dir, exist_ok=True)

for bin_name in binaries:
    print(f"Running {bin_name}...")
    result = subprocess.run(["cargo", "run", "--release", "--bin", bin_name], 
                            cwd=genesis_dir, capture_output=True, text=True)
    
    # Extract Master Hash
    match = re.search(r"Master Hash:\s*([a-f0-9]+)", result.stdout)
    if not match:
        print(f"Failed to find master hash for {bin_name}")
        print(result.stdout)
        continue
    
    master_hash = match.group(1)
    print(f"  -> Extracted Hash: {master_hash}")
    
    json_path = os.path.join(genesis_dir, f"{bin_name}_envelope.json")
    if not os.path.exists(json_path):
        # Some might have a slightly different name?
        print(f"Warning: JSON not found at {json_path}")
        continue
        
    with open(json_path, 'r') as f:
        data = json.load(f)
        
    # Wrap for ICP bridge
    wrapped = {
        "trajectories": [{"proof_hash": master_hash}],
        "data": data
    }
    
    dest_path = os.path.join(corpus_dir, f"{bin_name}_envelope.json")
    with open(dest_path, 'w') as f:
        json.dump(wrapped, f, indent=2)
        
    # Delete the old file so it's a "move"
    os.remove(json_path)
    print(f"  -> Moved to {dest_path}")

print("\nAll runs complete and moved. Now running ICP bridge...")
bridge_result = subprocess.run(["cargo", "run", "--release", "--bin", "icp_bridge"],
                               cwd=genesis_dir, capture_output=True, text=True)
print(bridge_result.stdout)
if bridge_result.stderr:
    print("Bridge stderr:", bridge_result.stderr)
