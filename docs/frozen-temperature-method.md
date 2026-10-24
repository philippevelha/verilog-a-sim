# Fast signals on a slow thermal node: the frozen-temperature method

A note on a method first used for the micro-ring PAM8 study
(`paper_writing/mrm_eqc/tests/t8_eye_power.py`, 2.42.0+11). It applies to any model where a
**slow state** (temperature, a bias-tee capacitor, a trap population) sets the operating
point of a **fast signal** (an eye diagram, a pulse response). It also applies when that slow
state has **several equilibria**.

## The problem

A ring modulator's temperature follows the circulating optical power through a thermal time
constant τ_th ≈ 1 µs. An eye-diagram transient lasts a few nanoseconds. Three things follow.

1. **The temperature is frozen during the transient.** It is whatever the DC operating point
   gave. If that point is solved at the first symbol's bias, the ring sits at the wrong
   temperature for the rest of the pattern.
2. **Self-heating can be bistable.** When the laser is on the red side of the resonance,
   heating pulls the resonance onto the laser: positive feedback. Above a threshold power the
   static equation has three roots. The middle one is unstable, and a real device never sits
   on it.
3. **The DC solve chooses a branch for you.** Newton started cold can land on the cold branch,
   or on the unstable middle root. A `.nodeset` on the thermal net alone does not help. The
   seeded temperature has no optical field to sustain it, because the field states start at
   zero and can't be named in a netlist, so Newton's first step erases the seed. This was
   measured: hot-root seeds of 7.96 K and 9.5 K gave the same 0.84 K point as no seed.

The symptom is an eye that seems to "collapse" above some power, with a best laser position
that jumps by hundreds of picometres. That is a solver artifact, not physics.

## The method

Separate the two time scales instead of asking one solve to handle both.

1. **Frozen state = parameter shift.** Over the transient, a constant self-heating shift d of
   the resonance is exactly a laser offset of −d. So the eye at power P and *effective*
   detuning δ (laser minus heated resonance) equals the eye with self-heating **off** at the
   same δ. Its height scales with P.
2. **One scan.** With self-heating off, scan δ at one power. That gives eye(δ). It is cheap and
   needs no branch selection.
3. **Stability per power.** The static heating law is d = k·P·L(δ), where L is the normalised
   intracavity (Lorentzian) power and k = dλ/dP_in (0.215 nm/mW here). The operating point is
   thermally stable iff k·P·dL/dδ > −1. On the blue side dL/dδ > 0, so it is always stable. On
   the red side it is stable only up to P = 1 / (k·|dL/dδ|).
4. **Best eye per power** = best eye(δ) over the stable δ. The laser that realises it is at
   λ_res,cold + d(δ) + δ. A real link reaches it with heater or laser tuning, approaching from
   the blue.
5. **Check it once** with a direct self-heating run at a few powers where the static equation
   has a single root, so the solver cannot pick a wrong branch. In the MRM case the direct runs
   matched the prediction to 0.5 %: 2.91/2.91, 5.79/5.82 and 11.10/11.11 µW at 0.5/1/2 mW.

## What it assumes

- **τ_slow ≫ the transient.** Check the ratio. Here it is 1 µs against 2.4 ns.
- **The static state is evaluated at the mean drive** (the pattern starts with a few symbols
  at the mean level, inside the eye metric's skip). The true average heating is the
  pattern-average absorbed power. The mean-drive value approximates it, and the difference
  grows with the nonlinearity of the transfer curve.
- **The stability criterion is the quasi-static one,** the slope of the static equation. It
  ignores any optical–thermal dynamics faster than τ_th, such as self-pulsing. Those need a
  long transient (see `docs/multiphysics.md`, "Silicon microring bank: optical bistability,
  self-pulsing and chaos").
- **A Python twin of the heating law** must match the model's. Here it is `t5.steady_T`, and
  the direct-run check in step 5 tests the match.

## When to reach for it

Use it when a sweep over a power-like parameter shows a sudden collapse, a best operating point
that jumps, or results that change with the solver's starting point. First count the static
roots against the swept parameter (step 3). If there is more than one, the solver's branch is
deciding the answer, and this method replaces that choice with a physical one.
