# Manifest: Spectre netlists for the circuits without a golden reference

103 translated (84 standard, 19 heavy), 37 left out (REFUSED.md).

| Netlist | Deck | Analyses | Models | Class | Instances |
|---|---|---|---|---|---|
| `actuator_plant` | `circuits/actuator_plant.net` | tran | models/vsin.va, models/actuator_plant.va, models/actuator.va, models/spring_load.va | standard | 2 |
| `benchmark__cap_fast_edge` | `circuits/benchmark/cap_fast_edge.net` | tran | built-ins only | standard | 4 |
| `benchmark__nand__bsim6` | `circuits/benchmark/nand/bsim6.net` | dc | benchmark/models/bsim6/bsim6.0.va | standard | 7 |
| `benchmark__nand__bsimbulk107` | `circuits/benchmark/nand/bsimbulk107.net` | dc | benchmark/models/bsimbulk/bsimbulk107.va | standard | 11 |
| `benchmark__nand__bsimcmg` | `circuits/benchmark/nand/bsimcmg.net` | dc | benchmark/models/bsimcmg/bsimcmg.va | standard | 11 |
| `benchmark__nand__bsimimg` | `circuits/benchmark/nand/bsimimg.net` | dc | benchmark/models/bsimimg/bsimimg.va | standard | 11 |
| `benchmark__nand__bsimsoi` | `circuits/benchmark/nand/bsimsoi.net` | dc | benchmark/models/bsimsoi/bsimsoi.va | standard | 7 |
| `benchmark__nand__hisim2` | `circuits/benchmark/nand/hisim2.net` | dc | benchmark/models/hisim2/hisim2.va | standard | 7 |
| `benchmark__nand__lutsoi` | `circuits/benchmark/nand/lutsoi.net` | dc | benchmark/models/L-UTSOI/L_UTSOI_102.va | standard | 11 |
| `benchmark__nand__psp102` | `circuits/benchmark/nand/psp102.net` | dc | benchmark/models/psp102/psp102.va | standard | 7 |
| `benchmark__nand__psp103` | `circuits/benchmark/nand/psp103.net` | dc | benchmark/models/psp103/psp103.va | standard | 7 |
| `benchmark__nand__psp104` | `circuits/benchmark/nand/psp104.net` | dc | benchmark/models/psp104/psp104.va | standard | 7 |
| `benchmark__nand__zoo_nmos` | `circuits/benchmark/nand/zoo_nmos.net` | dc | models/mosfet.va | standard | 7 |
| `benchmark__psp103_inverter_card_tran` | `circuits/benchmark/psp103_inverter_card_tran.net` | tran | benchmark/models/psp103/psp103.va | standard | 7 |
| `benchmark__psp103_inverter_card_vtc` | `circuits/benchmark/psp103_inverter_card_vtc.net` | dc | benchmark/models/psp103/psp103.va | standard | 7 |
| `benchmark__psp103_inverter_tran` | `circuits/benchmark/psp103_inverter_tran.net` | tran | benchmark/models/psp103/psp103.va | standard | 7 |
| `benchmark__psp103_inverter_vtc` | `circuits/benchmark/psp103_inverter_vtc.net` | dc | benchmark/models/psp103/psp103.va | standard | 7 |
| `delay_ac` | `circuits/delay_ac.net` | ac | models/delay_line.va | standard | 3 |
| `fundamental_diagram` | `circuits/fundamental_diagram.net` | dc | models/fundamental_diagram.va | standard | 2 |
| `hier_divider` | `circuits/hier_divider.net` | op | models/series_divider.va, models/leg.va | standard | 3 |
| `interferometer_ac` | `circuits/interferometer_ac.net` | ac | models/interferometer.va | standard | 3 |
| `lib_tee` | `circuits/lib_tee.net` | op | tests/fixtures/lib/tee.va, tests/fixtures/lib/scaler.va | standard | 5 |
| `multiphysics__breaker_interrupt` | `circuits/multiphysics/breaker_interrupt.net` | tran | models/multiphysics/breaker/breaker.va | standard | 9 |
| `multiphysics__breaker_static` | `circuits/multiphysics/breaker_static.net` | tran | models/multiphysics/breaker/breaker.va | standard | 4 |
| `multiphysics__bubble_cloud_check` | `circuits/multiphysics/bubble_cloud_check.net` | tran | models/multiphysics/bubble/bubble_parts.va | heavy | 400 |
| `multiphysics__bubble_cloud_profiles` | `circuits/multiphysics/bubble_cloud_profiles.net` | ac | models/multiphysics/bubble/bubble_parts.va | heavy | 400 |
| `multiphysics__bubble_cloud_response` | `circuits/multiphysics/bubble_cloud_response.net` | ac | models/multiphysics/bubble/bubble_parts.va | heavy | 400 |
| `multiphysics__bubble_cloud_sweep_w025` | `circuits/multiphysics/bubble_cloud_sweep_w025.net` | tran | models/multiphysics/bubble/bubble_parts.va | heavy | 400 |
| `multiphysics__bubble_cloud_sweep_w055` | `circuits/multiphysics/bubble_cloud_sweep_w055.net` | tran | models/multiphysics/bubble/bubble_parts.va | heavy | 400 |
| `multiphysics__bubble_cloud_sweep_w05_a01` | `circuits/multiphysics/bubble_cloud_sweep_w05_a01.net` | tran | models/multiphysics/bubble/bubble_parts.va | heavy | 400 |
| `multiphysics__bubble_cloud_sweep_w062` | `circuits/multiphysics/bubble_cloud_sweep_w062.net` | tran | models/multiphysics/bubble/bubble_parts.va | heavy | 400 |
| `multiphysics__bubble_cloud_sweep_w085` | `circuits/multiphysics/bubble_cloud_sweep_w085.net` | tran | models/multiphysics/bubble/bubble_parts.va | heavy | 400 |
| `multiphysics__bubble_cloud_sweep_w2` | `circuits/multiphysics/bubble_cloud_sweep_w2.net` | tran | models/multiphysics/bubble/bubble_parts.va | heavy | 400 |
| `multiphysics__bubble_linear` | `circuits/multiphysics/bubble_linear.net` | ac | models/multiphysics/bubble/bubble_parts.va | standard | 2 |
| `multiphysics__bubble_ramp` | `circuits/multiphysics/bubble_ramp.net` | tran | models/multiphysics/bubble/bubble_parts.va | standard | 2 |
| `multiphysics__bubble_single` | `circuits/multiphysics/bubble_single.net` | tran | models/multiphysics/bubble/bubble_parts.va | standard | 20 |
| `multiphysics__grid_cascade` | `circuits/multiphysics/grid_cascade.net` | tran | models/multiphysics/powergrid/grid_parts.va | standard | 19 |
| `multiphysics__grid_equal_area` | `circuits/multiphysics/grid_equal_area.net` | tran | models/multiphysics/powergrid/grid_parts.va | standard | 5 |
| `multiphysics__hh_giant_axon` | `circuits/multiphysics/hh_giant_axon.net` | tran | models/multiphysics/axon/hh_cable.va | standard | 5 |
| `multiphysics__hh_thin_fibre` | `circuits/multiphysics/hh_thin_fibre.net` | tran | models/multiphysics/axon/hh_cable.va | standard | 3 |
| `multiphysics__li_calendar` | `circuits/multiphysics/li_calendar.net` | tran | models/multiphysics/liion/li_parts.va | standard | 15 |
| `multiphysics__li_discharge` | `circuits/multiphysics/li_discharge.net` | tran | models/multiphysics/liion/li_parts.va | standard | 16 |
| `multiphysics__li_oven` | `circuits/multiphysics/li_oven.net` | tran | models/multiphysics/liion/li_parts.va | standard | 9 |
| `multiphysics__mems_hysteresis` | `circuits/multiphysics/mems_hysteresis.net` | tran | models/multiphysics/mems/mems_parts.va | standard | 7 |
| `multiphysics__mems_pullin` | `circuits/multiphysics/mems_pullin.net` | dc | models/multiphysics/mems/mems_parts.va | standard | 5 |
| `multiphysics__mems_resonance` | `circuits/multiphysics/mems_resonance.net` | ac | models/multiphysics/mems/mems_parts.va | standard | 11 |
| `multiphysics__optomech_limit_cycle` | `circuits/multiphysics/optomech_limit_cycle.net` | tran | models/multiphysics/optomech/optomech.va | heavy | 2 |
| `multiphysics__optomech_threshold` | `circuits/multiphysics/optomech_threshold.net` | tran | models/multiphysics/optomech/optomech.va | heavy | 8 |
| `multiphysics__piezo_duffing` | `circuits/multiphysics/piezo_duffing.net` | tran | models/multiphysics/piezo/piezo_parts.va | heavy | 5 |
| `multiphysics__piezo_linear` | `circuits/multiphysics/piezo_linear.net` | tran | models/multiphysics/piezo/piezo_parts.va | standard | 3 |
| `multiphysics__piezo_storage` | `circuits/multiphysics/piezo_storage.net` | tran | models/multiphysics/piezo/piezo_parts.va | standard | 60 |
| `multiphysics__relay_bh` | `circuits/multiphysics/relay_bh.net` | tran | models/multiphysics/relay/relay_parts.va | standard | 9 |
| `multiphysics__relay_switch` | `circuits/multiphysics/relay_switch.net` | tran | models/multiphysics/relay/relay_parts.va | standard | 12 |
| `multiphysics__reram_freq_500k` | `circuits/multiphysics/reram_freq_500k.net` | tran | models/multiphysics/reram/reram.va | standard | 3 |
| `multiphysics__reram_freq_5k` | `circuits/multiphysics/reram_freq_5k.net` | tran | models/multiphysics/reram/reram.va | standard | 3 |
| `multiphysics__reram_freq_5meg` | `circuits/multiphysics/reram_freq_5meg.net` | tran | models/multiphysics/reram/reram.va | standard | 3 |
| `multiphysics__reram_sweep` | `circuits/multiphysics/reram_sweep.net` | tran | models/multiphysics/reram/reram.va | standard | 5 |
| `multiphysics__ring_bank` | `circuits/multiphysics/ring_bank.net` | tran | models/multiphysics/ringbank/ring_parts.va | standard | 35 |
| `multiphysics__ring_selfheat` | `circuits/multiphysics/ring_selfheat.net` | tran | models/multiphysics/ringbank/ring_parts.va | standard | 11 |
| `multiphysics__ringchaos_bank` | `circuits/multiphysics/ringchaos_bank.net` | tran | models/multiphysics/ringchaos/rc_parts.va | heavy | 21 |
| `multiphysics__ringchaos_bistable` | `circuits/multiphysics/ringchaos_bistable.net` | tran | models/multiphysics/ringchaos/rc_parts.va | standard | 5 |
| `multiphysics__ringchaos_selfpulse` | `circuits/multiphysics/ringchaos_selfpulse.net` | tran | models/multiphysics/ringchaos/rc_parts.va | standard | 9 |
| `multiphysics__seu_collection` | `circuits/multiphysics/seu_collection.net` | tran | models/multiphysics/seu/seu_parts.va | standard | 2 |
| `multiphysics__seu_sram` | `circuits/multiphysics/seu_sram.net` | tran | models/multiphysics/seu/seu_parts.va | standard | 20 |
| `multiphysics__speaker_ac` | `circuits/multiphysics/speaker_ac.net` | ac | models/multiphysics/speaker/speaker_parts.va | standard | 13 |
| `multiphysics__speaker_large` | `circuits/multiphysics/speaker_large.net` | tran | models/multiphysics/speaker/speaker_parts.va | standard | 13 |
| `multiphysics__speaker_thermal` | `circuits/multiphysics/speaker_thermal.net` | tran | models/multiphysics/speaker/speaker_parts.va | standard | 7 |
| `multiphysics__tec_dtmax` | `circuits/multiphysics/tec_dtmax.net` | dc | models/multiphysics/tec/tec.va, models/multiphysics/tec/thermal_parts.va | standard | 3 |
| `multiphysics__tec_energy` | `circuits/multiphysics/tec_energy.net` | op | models/multiphysics/tec/tec.va, models/multiphysics/tec/thermal_parts.va | standard | 5 |
| `multiphysics__tec_pid` | `circuits/multiphysics/tec_pid.net` | tran | models/multiphysics/tec/tec.va, models/multiphysics/tec/pid.va, models/multiphysics/tec/thermal_parts.va | standard | 7 |
| `nlcap_ramp` | `circuits/nlcap_ramp.net` | tran | models/nlcap.va | standard | 3 |
| `portprobe_ac` | `circuits/portprobe_ac.net` | ac | models/portprobe_rc.va | standard | 2 |
| `portprobe_dc` | `circuits/portprobe_dc.net` | op | models/portprobe_rc.va | standard | 2 |
| `portprobe_ramp` | `circuits/portprobe_ramp.net` | tran | models/portprobe_rc.va | standard | 3 |
| `portprobe_sq` | `circuits/portprobe_sq.net` | tran | models/portprobe_sq.va | standard | 3 |
| `rc_pulse` | `circuits/rc_pulse.net` | tran | built-ins only | standard | 3 |
| `selfprobe_ramp` | `circuits/selfprobe_ramp.net` | tran | models/selfprobe_rc.va | standard | 3 |
| `snn__fig4_neuron` | `circuits/snn/fig4_neuron.net` | tran | models/snn/vswitch.va, models/snn/comparator.va, models/snn/rram.va, models/snn/sl_stage.va, models/snn/neuron_logic.va | standard | 52 |
| `snn__fig5_neuron` | `circuits/snn/fig5_neuron.net` | tran | models/snn/vswitch.va, models/snn/comparator.va, models/snn/rram.va, models/snn/sl_stage.va, models/snn/neuron_logic.va | standard | 51 |
| `snn__transistor__bench_access_bias` | `circuits/snn/transistor/bench_access_bias.net` | op | models/sg13g2/psp103/psp103.va | standard | 6 |
| `snn__transistor__fig5_access` | `circuits/snn/transistor/fig5_access.net` | tran | models/sg13g2/psp103/psp103.va, models/snn/vswitch.va, models/snn/comparator.va, models/snn/rram.va, models/snn/sl_stage.va, models/snn/neuron_logic.va | standard | 51 |
| `snn__transistor__fig5_fit` | `circuits/snn/transistor/fig5_fit.net` | tran | models/snn/vswitch.va, models/snn/comparator.va, models/snn/rram.va, models/snn/mos_switch.va, models/snn/sl_stage.va, models/snn/neuron_logic.va | standard | 51 |
| `snn__transistor__fig5_fitbc` | `circuits/snn/transistor/fig5_fitbc.net` | tran | models/snn/vswitch.va, models/snn/comparator.va, models/snn/rram.va, models/snn/mos_switch.va, models/snn/sl_stage.va, models/snn/neuron_logic.va | standard | 51 |
| `snn__transistor__fig5_fitc` | `circuits/snn/transistor/fig5_fitc.net` | tran | models/snn/vswitch.va, models/snn/comparator.va, models/snn/rram.va, models/snn/mos_switch.va, models/snn/sl_stage.va, models/snn/neuron_logic.va | standard | 51 |
| `snn__transistor__fig5_tx` | `circuits/snn/transistor/fig5_tx.net` | tran | models/sg13g2/psp103/psp103.va, models/snn/vswitch.va, models/snn/rram.va | standard | 109 |
| `snn__transistor__fig5_tx_cmp` | `circuits/snn/transistor/fig5_tx_cmp.net` | tran | models/sg13g2/psp103/psp103.va, models/snn/vswitch.va, models/snn/rram.va, models/snn/sl_stage.va, models/snn/neuron_logic.va | standard | 58 |
| `snn__transistor__fig5_tx_dff` | `circuits/snn/transistor/fig5_tx_dff.net` | tran | models/sg13g2/psp103/psp103.va, models/snn/vswitch.va, models/snn/rram.va, models/snn/sl_stage.va | standard | 80 |
| `snn__transistor__fig5_tx_in` | `circuits/snn/transistor/fig5_tx_in.net` | tran | models/sg13g2/psp103/psp103.va, models/snn/vswitch.va, models/snn/rram.va, models/snn/comparator.va, models/snn/neuron_logic.va | standard | 78 |
| `snn__transistor__fig5_tx_none` | `circuits/snn/transistor/fig5_tx_none.net` | tran | models/sg13g2/psp103/psp103.va, models/snn/vswitch.va, models/snn/rram.va, models/snn/sl_stage.va, models/snn/comparator.va, models/snn/neuron_logic.va | standard | 49 |
| `snn__transistor__fig5_tx_sw` | `circuits/snn/transistor/fig5_tx_sw.net` | tran | models/sg13g2/psp103/psp103.va, models/snn/vswitch.va, models/snn/rram.va, models/snn/sl_stage.va, models/snn/comparator.va, models/snn/neuron_logic.va | standard | 50 |
| `snn__transistor__mnist_tx_digit0` | `circuits/snn/transistor/mnist_tx_digit0.net` | tran | models/sg13g2/psp103/psp103.va, models/snn/rram.va | heavy | 9330 |
| `snn__transistor__mnist_tx_digit0_5ns` | `circuits/snn/transistor/mnist_tx_digit0_5ns.net` | tran | models/sg13g2/psp103/psp103.va, models/snn/rram.va | heavy | 9330 |
| `snn__transistor__scan_input_stage` | `circuits/snn/transistor/scan_input_stage.net` | dc | models/sg13g2/psp103/psp103.va | standard | 34 |
| `snn__transistor__uic_access` | `circuits/snn/transistor/uic_access.net` | tran | models/sg13g2/psp103/psp103.va, models/snn/rram.va | standard | 5 |
| `snn__transistor__xbar_h2_16` | `circuits/snn/transistor/xbar_h2_16.net` | tran | models/sg13g2/psp103/psp103.va, models/snn/rram.va, models/snn/vswitch.va, models/snn/sl_stage.va, models/snn/comparator.va, models/snn/neuron_logic.va | heavy | 1239 |
| `snn__transistor__xbar_h2_32` | `circuits/snn/transistor/xbar_h2_32.net` | tran | models/sg13g2/psp103/psp103.va, models/snn/rram.va, models/snn/vswitch.va, models/snn/sl_stage.va, models/snn/comparator.va, models/snn/neuron_logic.va | heavy | 2279 |
| `snn__transistor__xbar_h2_64` | `circuits/snn/transistor/xbar_h2_64.net` | tran | models/sg13g2/psp103/psp103.va, models/snn/rram.va, models/snn/vswitch.va, models/snn/sl_stage.va, models/snn/comparator.va, models/snn/neuron_logic.va | heavy | 4359 |
| `snn__transistor__xbar_h2_8` | `circuits/snn/transistor/xbar_h2_8.net` | tran | models/sg13g2/psp103/psp103.va, models/snn/rram.va, models/snn/vswitch.va, models/snn/sl_stage.va, models/snn/comparator.va, models/snn/neuron_logic.va | heavy | 719 |
| `tasks__current_limit` | `circuits/tasks/current_limit.net` | dc | models/tasks/current_limit.va | standard | 2 |
| `tasks__rc_logger` | `circuits/tasks/rc_logger.net` | tran | models/tasks/rc_logger.va | standard | 4 |
| `tasks__rc_watch` | `circuits/tasks/rc_watch.net` | tran | models/tasks/rc_watch.va | standard | 4 |
| `thermistor_rt` | `circuits/thermistor_rt.net` | dc | models/thermistor.va | standard | 3 |
| `transformer` | `circuits/transformer.net` | tran | built-ins only | standard | 5 |
