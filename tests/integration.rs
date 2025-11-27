use litesvm::LiteSVM;
use solana_sdk::{
    instruction::{AccountMeta, Instruction},
    message::Message,
    native_token::LAMPORTS_PER_SOL,
    pubkey::Pubkey,
    transaction::Transaction,
};
use solana_sdk_ids::system_program;
use solana_system_interface::instruction::create_account;

const PROGRAM_ID: Pubkey = Pubkey::new_from_array([5; 32]);
const TOKEN_PROGRAM: Pubkey = solana_sdk::pubkey!("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA");

#[test]
fn integration() {
    let mut svm = LiteSVM::new()
        .with_sigverify(false)
        .with_blockhash_check(false)
        .with_transaction_history(0);
    svm.add_program_from_file(PROGRAM_ID, "target/deploy/introspectardio.so")
        .unwrap();
    svm.add_program_from_file(TOKEN_PROGRAM, "ptoken.so")
        .unwrap();

    let payer = Pubkey::new_unique();
    let user = Pubkey::new_unique();

    let mint_a = Pubkey::new_unique(); // SOL-wrapped (decimals=9)
    let mint_b = Pubkey::new_unique(); // USDC (decimals=6)

    svm.airdrop(&payer, 100 * LAMPORTS_PER_SOL).unwrap();
    svm.airdrop(&user, 100 * LAMPORTS_PER_SOL).unwrap();

    // derive vault PDAs (seeds: pool_key, mint_key)
    let (pool, _) = Pubkey::find_program_address(&[mint_a.as_ref(), mint_b.as_ref()], &PROGRAM_ID);
    let (vault_a, _) = Pubkey::find_program_address(&[pool.as_ref(), mint_a.as_ref()], &PROGRAM_ID);
    let (vault_b, _) = Pubkey::find_program_address(&[pool.as_ref(), mint_b.as_ref()], &PROGRAM_ID);


    // create mints
    create_mint(&mut svm, &payer, &mint_a, 9); // SOL decimals
    create_mint(&mut svm, &payer, &mint_b, 6); // USDC decimals

    // create user token accounts
    let user_ata_a = Pubkey::new_unique();
    let user_ata_b = Pubkey::new_unique();
    create_token_account(&mut svm, &payer, &user_ata_a, &mint_a, &user);
    create_token_account(&mut svm, &payer, &user_ata_b, &mint_b, &user);

    // mint some tokens to user
    mint_to(
        &mut svm,
        &payer,
        &mint_a,
        &user_ata_a,
        10 * LAMPORTS_PER_SOL,
    );

    // 1) Initialize pool
    let usdc_atoms_per_sol: u64 = 1_000 * 1_000_000; // $1000 per SOL in USDC atoms (6 decimals)
    let mut init_data = vec![0u8]; // discriminator
    init_data.extend_from_slice(&usdc_atoms_per_sol.to_le_bytes());

    let init_ixn = Instruction {
        program_id: PROGRAM_ID,
        accounts: vec![
            AccountMeta::new(payer, true),
            AccountMeta::new(pool, false),
            AccountMeta::new(vault_a, false),
            AccountMeta::new(vault_b, false),
            AccountMeta::new_readonly(mint_a, false),
            AccountMeta::new_readonly(mint_b, false),
            AccountMeta::new_readonly(system_program::ID, false),
            AccountMeta::new_readonly(TOKEN_PROGRAM, false),
        ],
        data: init_data,
    };

    let msg = Message::new(&[init_ixn], Some(&payer));
    let txn = Transaction::new_unsigned(msg);
    let res = svm.send_transaction(txn).unwrap();

    println!("Initialize Pool");
    for log in res.logs {
        println!("    {log}");
    }

    // mint USDC to vault_b so pool has liquidity
    mint_to(&mut svm, &payer, &mint_b, &vault_b, 1_000_000 * 1_000_000); // 1M USDC (6 decimals)

    // 2) Swap: user transfers SOL-token to vault_a, then calls swap
    let amount_in: u64 = 1 * LAMPORTS_PER_SOL; // 1 SOL (9 decimals)

    // transfer instruction (user -> vault_a)
    let transfer_ixn = spl_token_transfer_instruction(&user_ata_a, &vault_a, &user, amount_in);

    // swap instruction
    let swap_ixn = Instruction {
        program_id: PROGRAM_ID,
        accounts: vec![
            AccountMeta::new(user, true),
            AccountMeta::new_readonly(pool, false),
            AccountMeta::new(user_ata_b, false), // user_out
            AccountMeta::new(vault_a, false),    // pool_vault_a
            AccountMeta::new(vault_b, false),    // pool_vault_b
            AccountMeta::new_readonly(solana_sdk::sysvar::instructions::ID, false),
            AccountMeta::new_readonly(TOKEN_PROGRAM, false),
        ],
        data: vec![1], // swap discriminator
    };

    let before_user_b = get_token_balance(&svm, &user_ata_b);

    let msg = Message::new(&[transfer_ixn, swap_ixn], Some(&user));
    let txn = Transaction::new_unsigned(msg);
    let res = svm.send_transaction(txn).unwrap();

    println!("Swap");
    for log in res.logs {
        println!("    {log}");
    }

    let after_user_b = get_token_balance(&svm, &user_ata_b);

    // expected: 1 SOL * 1000 USDC/SOL = 1000 USDC = 1_000_000_000 USDC atoms (6 decimals)
    let expected_out = amount_in * usdc_atoms_per_sol / LAMPORTS_PER_SOL;
    assert_eq!(after_user_b - before_user_b, expected_out);

    println!(
        "Swap successful: {} SOL -> {} USDC atoms",
        amount_in, expected_out
    );
}

fn create_mint(svm: &mut LiteSVM, payer: &Pubkey, mint: &Pubkey, decimals: u8) {
    let mint_space = 82; // Mint::LEN
    let rent = svm.minimum_balance_for_rent_exemption(mint_space);
    let create_ixn = create_account(payer, mint, rent, mint_space as u64, &TOKEN_PROGRAM);

    // InitializeMint2: disc=20, decimals, mint_authority, freeze_authority_option=0
    let mut init_data = vec![20u8, decimals];
    init_data.extend_from_slice(payer.as_ref()); // mint authority
    init_data.push(0); // no freeze authority

    let init_ixn = Instruction {
        program_id: TOKEN_PROGRAM,
        accounts: vec![AccountMeta::new(*mint, false)],
        data: init_data,
    };

    let msg = Message::new(&[create_ixn, init_ixn], Some(payer));
    let txn = Transaction::new_unsigned(msg);
    svm.send_transaction(txn).unwrap();
}

fn create_token_account(
    svm: &mut LiteSVM,
    payer: &Pubkey,
    account: &Pubkey,
    mint: &Pubkey,
    owner: &Pubkey,
) {
    let account_space = 165; // TokenAccount::LEN
    let rent = svm.minimum_balance_for_rent_exemption(account_space);
    let create_ixn = create_account(payer, account, rent, account_space as u64, &TOKEN_PROGRAM);

    // InitializeAccount3: disc=18, owner
    let mut init_data = vec![18u8];
    init_data.extend_from_slice(owner.as_ref());

    let init_ixn = Instruction {
        program_id: TOKEN_PROGRAM,
        accounts: vec![
            AccountMeta::new(*account, false),
            AccountMeta::new_readonly(*mint, false),
        ],
        data: init_data,
    };

    let msg = Message::new(&[create_ixn, init_ixn], Some(payer));
    let txn = Transaction::new_unsigned(msg);
    svm.send_transaction(txn).unwrap();
}

fn mint_to(svm: &mut LiteSVM, authority: &Pubkey, mint: &Pubkey, dest: &Pubkey, amount: u64) {
    // MintTo: disc=7, amount
    let mut data = vec![7u8];
    data.extend_from_slice(&amount.to_le_bytes());

    let ixn = Instruction {
        program_id: TOKEN_PROGRAM,
        accounts: vec![
            AccountMeta::new(*mint, false),
            AccountMeta::new(*dest, false),
            AccountMeta::new_readonly(*authority, true),
        ],
        data,
    };

    let msg = Message::new(&[ixn], Some(authority));
    let txn = Transaction::new_unsigned(msg);
    svm.send_transaction(txn).unwrap();
}

fn spl_token_transfer_instruction(
    from: &Pubkey,
    to: &Pubkey,
    authority: &Pubkey,
    amount: u64,
) -> Instruction {
    // Transfer: disc=3, amount
    let mut data = vec![3u8];
    data.extend_from_slice(&amount.to_le_bytes());

    Instruction {
        program_id: TOKEN_PROGRAM,
        accounts: vec![
            AccountMeta::new(*from, false),
            AccountMeta::new(*to, false),
            AccountMeta::new_readonly(*authority, true),
        ],
        data,
    }
}

fn get_token_balance(svm: &LiteSVM, account: &Pubkey) -> u64 {
    let acc = svm.get_account(account).unwrap();
    // amount is at offset 64 in TokenAccount
    unsafe { acc.data.as_ptr().add(64).cast::<u64>().read_unaligned() }
}
