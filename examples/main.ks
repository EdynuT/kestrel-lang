proc main() {
    // prints
    print("Meu nome e Edynu\n");
    print("Estou aprendendo Kestrel\n");

    // declaração de variaveis
    variavel1: int = 5;
    printf("exemplo de %%t = %t = 5\n", variavel1);
    printf("exemplo de %%v = %v = 5\n", variavel1);
    variavel1 = 15;
    printf("\n%i\n", variavel1);

    // operações básicas
    final variavel2: int = 2;
    printl(variavel2 + 2);
    printl(variavel2 - 2);
    printl(variavel2 * 2);
    printl(variavel2 / 2);

    variavel3: int = variavel2 + variavel1;
    printl(variavel3);
    variavel3: int = variavel2 - variavel1;
    printl(variavel3);
    variavel3: int = variavel2 * variavel1;
    printl(variavel3);
    variavel3: real = (real) variavel2 / (real) variavel1;
    printl(variavel3);


    // comparações
    if variavel1 < variavel2 {
        printf("{variavel1} é menor que {variavel2}\n");
    } else if (real) variavel2 < variavel3 {
        printf("{variavel2} é maior que {variavel1} e menor que {variavel3}\n");
    } else if (real) variavel1 < variavel3 {
        printf("{variavel1} é menor que {variavel3}\n");
    } else {
        printf("{variavel3} é minúsculo\n");
    }

    // loop de repetição
    a: int = 0;
    while a < 10 {
        printl(a);
        a++;
    }

    // switch case do java, ao menos parece
    mes: int = 12;
    dias: int = 0;
    match mes {
        2 => dias = 28;
        4 | 6 | 9 | 11 => dias = 30;
        else => dias = 31;
    }
    printf("O mês %d tem %d dias, muita curtição!\n", mes, dias);

    // arrays simétricos e assimétricos com laço de repetição usando for e walk
    sym arr[]: int = {1, 2, 3};
    walk i in span 1 {
        printf("%d ", arr[i]);
    }
    burn arr;
    print("\n");

    arr[]: int = {1, 2, 3, 4};
    for i: int = 0; i < len(arr); i++ {
        printf("%d ", arr[i]);
    }
    burn arr;
    print("\n");
}
