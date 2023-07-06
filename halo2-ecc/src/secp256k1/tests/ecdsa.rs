#![allow(non_snake_case)]
use ark_std::{end_timer, start_timer};
use halo2_base::utils::PrimeField;
use std::marker::PhantomData;
use std::{env::var, io::Write};

use crate::halo2_proofs::{
    arithmetic::CurveAffine,
    dev::MockProver,
    halo2curves::bn256::{Bn256, Fr, G1Affine},
    halo2curves::secp256k1::{Fq, Secp256k1Affine},
    plonk::*,
    transcript::{Blake2bRead, Blake2bWrite, Challenge255},
};
use rand_core::OsRng;

use halo2_base::utils::{biguint_to_fe, fe_to_biguint, modulus};

use crate::secp256k1::ecdsa::{CircuitParams, ECDSACircuit};

#[cfg(test)]
#[test]
fn test_secp256k1_ecdsa() {
    use halo2_base::{utils::fs::gen_srs, halo2_proofs::{transcript::{TranscriptWriterBuffer, TranscriptReadBuffer}, poly::kzg::{commitment::KZGCommitmentScheme, multiopen::{ProverSHPLONK, VerifierSHPLONK}, strategy::SingleStrategy}, halo2curves::{serde::SerdeObject, secp256k1::Fp}}};
    use num_bigint::BigUint;
    use num_traits::FromPrimitive;

    let mut folder = std::path::PathBuf::new();
    folder.push("./src/secp256k1");
    folder.push("configs/ecdsa_circuit.config");
    let params_str = std::fs::read_to_string(folder.as_path())
        .expect("src/secp256k1/configs/ecdsa_circuit.config file should exist");
    let params: CircuitParams = serde_json::from_str(params_str.as_str()).unwrap();
    let K = params.degree;

    // generate random pub key and sign random message
    // NOTE (Wentao XIAO) G and pubkey is generated in Sibyl and should be exposed to client and stored in public IO
    // G (base point) is always the same for secp256k1: (0x79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798,
    // 0x483ada7726a3c4655da4fbfc0e1108a8fd17b448a68554199c47d08ffb10d4b8)
    let G = Secp256k1Affine::generator();
    // private key
    let sk = <Secp256k1Affine as CurveAffine>::ScalarExt::random(OsRng);
    // TODO (Wentao XIAO) pubkey can be compressed
    let pubkey = Secp256k1Affine::from(G * sk);
    // let msg_hash = <Secp256k1Affine as CurveAffine>::ScalarExt::random(OsRng);
    let msg_hash = biguint_to_fe::<Fq>(&BigUint::from_u32(52u32).unwrap());

    // NOTE (Wentao XIAO) k is the random number, should not be exposed to any party outside the Sibyl
    let k = <Secp256k1Affine as CurveAffine>::ScalarExt::random(OsRng);
    let k_inv = k.invert().unwrap();

    // NOTE (Wentao XIAO) signature `s` and `r` are generated in Sibyl
    let r_point = Secp256k1Affine::from(G * k).coordinates().unwrap();
    let x = r_point.x();
    let x_bigint = fe_to_biguint(x);
    let r = biguint_to_fe::<Fq>(&(x_bigint % modulus::<Fq>()));
    let s = k_inv * (msg_hash + (r * sk));

    println!("######## r s pubkey G: {:?} {:?} {:?} {:?}", r, s, pubkey, G);

    let lower = 50;
    let upper = 100;
    let circuit = ECDSACircuit::<Fr> {
        r: Some(r),
        s: Some(s),
        lower: Some(biguint_to_fe::<Fq>(&BigUint::from_u32(lower).unwrap())),
        upper: Some(biguint_to_fe::<Fq>(&BigUint::from_u32(upper).unwrap())),
        msghash: Some(msg_hash),
        pk: Some(pubkey),
        G,
        _marker: PhantomData,
    };
    println!("public x: {:?}, y: {:?}", pubkey.x, pubkey.y);
    let public_io = vec![
        biguint_to_fe::<Fr>(&BigUint::from_u32(lower).unwrap()), biguint_to_fe::<Fr>(&BigUint::from_u32(upper).unwrap())];
    println!("public io: {:?}", public_io);

    // let prover = MockProver::run(K, &circuit, vec![public_io]).unwrap();
    //prover.assert_satisfied();
    // assert_eq!(prover.verify(), Ok(()));

    let params = gen_srs(K as u32);
    let mut transcript: Blake2bWrite<Vec<u8>, G1Affine, Challenge255<G1Affine>> = Blake2bWrite::<_, _, Challenge255<_>>::init(vec![]);
    let vk = keygen_vk(&params, &circuit).expect("vk generation failed");
    let pk = keygen_pk(&params, vk, &circuit).expect("pk generation failed");
    create_proof::<
        KZGCommitmentScheme<Bn256>,
        ProverSHPLONK<'_, Bn256>,
        Challenge255<G1Affine>,
        _,
        Blake2bWrite<Vec<u8>, G1Affine, Challenge255<G1Affine>>,
        _,
    >(&params, &pk, &[circuit], &[&[&public_io]], OsRng, &mut transcript)
        .expect("proof generation failed");
    let proof = transcript.finalize();
    let strategy = SingleStrategy::new(&params);
    let mut transcript = Blake2bRead::<_, _, Challenge255<_>>::init(&proof[..]);
    verify_proof::<
        KZGCommitmentScheme<Bn256>,
        VerifierSHPLONK<'_, Bn256>,
        Challenge255<G1Affine>,
        Blake2bRead<&[u8], G1Affine, Challenge255<G1Affine>>,
        SingleStrategy<'_, Bn256>,
    >(&params, pk.get_vk(), strategy, &[&[&public_io]], &mut transcript).unwrap();
}

#[cfg(test)]
#[test]
fn bench_secp256k1_ecdsa() -> Result<(), Box<dyn std::error::Error>> {
    use num_bigint::BigUint;
    use num_traits::FromPrimitive;

    use crate::halo2_proofs::{
        poly::commitment::{Params, ParamsProver},
        poly::kzg::{
            commitment::{KZGCommitmentScheme, ParamsKZG},
            multiopen::{ProverSHPLONK, VerifierSHPLONK},
            strategy::SingleStrategy,
        },
        transcript::{TranscriptReadBuffer, TranscriptWriterBuffer},
    };
    use std::fs::File;
    use std::io::{BufRead, BufReader};
    use std::{env::set_var, fs};

    let _rng = OsRng;

    let mut folder = std::path::PathBuf::new();
    folder.push("./src/secp256k1");

    folder.push("configs/bench_ecdsa.config");
    let bench_params_file = std::fs::File::open(folder.as_path()).unwrap();
    folder.pop();
    folder.pop();

    folder.push("results/ecdsa_bench.csv");
    let mut fs_results = std::fs::File::create(folder.as_path()).unwrap();
    folder.pop();
    folder.pop();
    writeln!(fs_results, "degree,num_advice,num_lookup,num_fixed,lookup_bits,limb_bits,num_limbs,proof_time,proof_size,verify_time")?;
    folder.push("data");
    if !folder.is_dir() {
        std::fs::create_dir(folder.as_path())?;
    }

    let bench_params_reader = std::io::BufReader::new(bench_params_file);
    for line in bench_params_reader.lines() {
        let bench_params: CircuitParams = serde_json::from_str(line.unwrap().as_str()).unwrap();
        let k = bench_params.degree;
        println!("---------------------- degree = {} ------------------------------", k);

        {
            folder.pop();
            folder.push("configs/ecdsa_circuit.tmp.config");
            set_var("ECDSA_CONFIG", &folder);
            let mut f = std::fs::File::create(folder.as_path())?;
            write!(f, "{}", serde_json::to_string(&bench_params).unwrap())?;
            folder.pop();
            folder.pop();
            folder.push("keys")
        }
        let params_time = start_timer!(|| "Time elapsed in circuit & params construction");
        let dir = "./params".to_string();
        let params = ParamsKZG::<Bn256>::read(&mut BufReader::new(
            File::open(format!("{dir}/kzg_bn254_{k}.srs").as_str())
                .expect("Params file does not exist"),
        ))
        .unwrap();
        println!("k, n of params: {}, {}", params.k(), params.n());
        let circuit = ECDSACircuit::<Fr>::default();
        end_timer!(params_time);

        let vk_time = start_timer!(|| "Time elapsed in generating vkey");
        let vk = keygen_vk(&params, &circuit)?;
        end_timer!(vk_time);

        // // write the verifying key to a file
        // {
        //     folder.push(format!("ecdsa_{}.vk", bench_params.degree));
        //     let f = std::fs::File::create(folder.as_path()).unwrap();
        //     let mut writer = BufWriter::new(f);
        //     vk.write(&mut writer, SerdeFormat::RawBytes).unwrap();
        //     writer.flush().unwrap();
        //     folder.pop();
        // }
        folder.pop();
        folder.push("data");

        let pk_time = start_timer!(|| "Time elapsed in generating pkey");
        let pk = keygen_pk(&params, vk, &circuit)?;
        end_timer!(pk_time);

        // write the proving key to a file
        // {
        //     folder.push(format!("ecdsa_{}.pk", bench_params.degree));
        //     let f = std::fs::File::create(folder.as_path()).unwrap();
        //     let mut writer = BufWriter::new(f);
        //     pk.write(&mut writer, SerdeFormat::RawBytes).unwrap();
        //     writer.flush().unwrap();
        //     folder.pop();
        // }

        // generate random pub key and sign random message
        let G = Secp256k1Affine::generator();
        let sk = <Secp256k1Affine as CurveAffine>::ScalarExt::random(OsRng);
        let pubkey = Secp256k1Affine::from(G * sk);
        // let msg_hash = <Secp256k1Affine as CurveAffine>::ScalarExt::random(OsRng);
        let msg_hash = biguint_to_fe::<Fq>(&BigUint::from_u32(52u32).unwrap());

        let k = <Secp256k1Affine as CurveAffine>::ScalarExt::random(OsRng);
        let k_inv = k.invert().unwrap();

        let r_point = Secp256k1Affine::from(G * k).coordinates().unwrap();
        let x = r_point.x();
        let x_bigint = fe_to_biguint(x);
        let r = biguint_to_fe::<Fq>(&x_bigint);
        let s = k_inv * (msg_hash + (r * sk));

        let proof_circuit = ECDSACircuit::<Fr> {
            r: Some(r),
            s: Some(s),
            lower: Some(biguint_to_fe::<Fq>(&BigUint::from_u32(50).unwrap())),
            upper: Some(biguint_to_fe::<Fq>(&BigUint::from_u32(100).unwrap())),
            msghash: Some(msg_hash),
            pk: Some(pubkey),
            G,
            _marker: PhantomData,
        };
        let mut rng = OsRng;

        // create a proof
        let proof_time = start_timer!(|| "Proving time");
        let mut transcript = Blake2bWrite::<_, _, Challenge255<_>>::init(vec![]);
        create_proof::<
            KZGCommitmentScheme<Bn256>,
            ProverSHPLONK<'_, Bn256>,
            Challenge255<G1Affine>,
            _,
            Blake2bWrite<Vec<u8>, G1Affine, Challenge255<G1Affine>>,
            ECDSACircuit<Fr>,
        >(&params, &pk, &[proof_circuit], &[&[]], &mut rng, &mut transcript)?;
        let proof = transcript.finalize();
        end_timer!(proof_time);

        let proof_size = {
            folder.push(format!(
                "ecdsa_circuit_proof_{}_{}_{}_{}_{}_{}_{}.data",
                bench_params.degree,
                bench_params.num_advice,
                bench_params.num_lookup_advice,
                bench_params.num_fixed,
                bench_params.lookup_bits,
                bench_params.limb_bits,
                bench_params.num_limbs
            ));
            let mut fd = std::fs::File::create(folder.as_path()).unwrap();
            folder.pop();
            fd.write_all(&proof).unwrap();
            fd.metadata().unwrap().len()
        };

        let verify_time = start_timer!(|| "Verify time");
        let verifier_params = params.verifier_params();
        let strategy = SingleStrategy::new(&params);
        let mut transcript = Blake2bRead::<_, _, Challenge255<_>>::init(&proof[..]);
        assert!(verify_proof::<
            KZGCommitmentScheme<Bn256>,
            VerifierSHPLONK<'_, Bn256>,
            Challenge255<G1Affine>,
            Blake2bRead<&[u8], G1Affine, Challenge255<G1Affine>>,
            SingleStrategy<'_, Bn256>,
        >(verifier_params, pk.get_vk(), strategy, &[&[]], &mut transcript)
        .is_ok());
        end_timer!(verify_time);
        fs::remove_file(var("ECDSA_CONFIG").unwrap())?;

        writeln!(
            fs_results,
            "{},{},{},{},{},{},{},{:?},{},{:?}",
            bench_params.degree,
            bench_params.num_advice,
            bench_params.num_lookup_advice,
            bench_params.num_fixed,
            bench_params.lookup_bits,
            bench_params.limb_bits,
            bench_params.num_limbs,
            proof_time.time.elapsed(),
            proof_size,
            verify_time.time.elapsed()
        )?;
    }
    Ok(())
}
